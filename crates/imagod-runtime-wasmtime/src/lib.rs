//! Wasmtime runtime integration used by runner processes.

pub mod native_plugins;

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use imago_protocol::ErrorCode;
use imagod_ipc::{
    CapabilityPolicy, PluginDependency, PluginKind, RunnerAppType, RunnerSocketConfig,
    RunnerSocketDirection,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Func, Linker, ResourceTable, types},
};
use wasmtime_wasi::{
    WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
    p2::{add_to_linker_async, bindings::Command},
    sockets::SocketAddrUse,
};
use wasmtime_wasi_http::{
    WasiHttpCtx, WasiHttpView, add_only_http_to_linker_async, bindings::Proxy,
    bindings::http::types::Scheme,
};

use imagod_common::ImagodError;
use imagod_runtime_internal::{
    ComponentRuntime, RuntimeHttpFuture, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeRunFuture,
    RuntimeRunRequest,
};

const STAGE_RUNTIME: &str = "runtime.start";
const HTTP_REQUEST_QUEUE_CAPACITY: usize = 32;

pub use native_plugins::{NativePlugin, NativePluginRegistry, NativePluginRegistryBuilder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePluginContext {
    service_name: String,
    release_hash: String,
    runner_id: String,
    app_type: String,
}

impl NativePluginContext {
    pub fn new(
        service_name: String,
        release_hash: String,
        runner_id: String,
        app_type: RunnerAppType,
    ) -> Self {
        Self {
            service_name,
            release_hash,
            runner_id,
            app_type: app_type_text(app_type).to_string(),
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn release_hash(&self) -> &str {
        &self.release_hash
    }

    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    pub fn app_type(&self) -> &str {
        &self.app_type
    }
}

pub fn app_type_text(app_type: RunnerAppType) -> &'static str {
    match app_type {
        RunnerAppType::Cli => "cli",
        RunnerAppType::Http => "http",
        RunnerAppType::Socket => "socket",
    }
}

/// Internal WASI host state stored in the Wasmtime store.
pub struct WasiState {
    table: ResourceTable,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    native_plugin_context: NativePluginContext,
}

impl WasiState {
    pub fn native_plugin_context(&self) -> &NativePluginContext {
        &self.native_plugin_context
    }
}

impl WasiView for WasiState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for WasiState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

#[derive(Clone)]
/// Runner-local wrapper around a configured Wasmtime engine.
pub struct WasmRuntime {
    engine: Arc<Engine>,
    http_instance: Arc<tokio::sync::Mutex<Option<RunningHttpComponent>>>,
    native_plugins: NativePluginRegistry,
}

/// Runtime state used while one HTTP component is running.
struct RunningHttpComponent {
    request_tx: mpsc::Sender<HttpWorkerRequest>,
    worker_task: JoinHandle<()>,
}

/// Ingress-to-worker payload for one incoming HTTP request.
struct HttpWorkerRequest {
    request: RuntimeHttpRequest,
    response_tx: oneshot::Sender<Result<RuntimeHttpResponse, ImagodError>>,
}

impl WasmRuntime {
    /// Creates a runtime with component model, async support, and epoch interruption enabled.
    pub fn new() -> Result<Self, ImagodError> {
        Self::new_with_native_plugins(NativePluginRegistry::default())
    }

    /// Creates a runtime with a native plugin registry injected by manager build.
    pub fn new_with_native_plugins(
        native_plugins: NativePluginRegistry,
    ) -> Result<Self, ImagodError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        config.epoch_interruption(true);

        let engine = Engine::new(&config)
            .map_err(|e| map_runtime_error(format!("engine init failed: {e}")))?;

        Ok(Self {
            engine: Arc::new(engine),
            http_instance: Arc::new(tokio::sync::Mutex::new(None)),
            native_plugins,
        })
    }

    /// Increments the engine epoch to unblock interruption-aware execution.
    pub fn increment_epoch(&self) {
        self.engine.increment_epoch();
    }

    fn validate_component_loadable(&self, component_path: &Path) -> Result<(), ImagodError> {
        Component::from_file(&self.engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;
        Ok(())
    }

    fn build_store(
        &self,
        args: &[String],
        envs: &BTreeMap<String, String>,
        socket: Option<&RunnerSocketConfig>,
        native_plugin_context: NativePluginContext,
    ) -> Result<Store<WasiState>, ImagodError> {
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdio();
        if !args.is_empty() {
            builder.args(args);
        }
        if !envs.is_empty() {
            let vars = envs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>();
            builder.envs(&vars);
        }
        if let Some(socket) = socket {
            configure_socket_policy(&mut builder, socket)?;
        }

        let state = WasiState {
            table: ResourceTable::new(),
            wasi: builder.build(),
            http: WasiHttpCtx::new(),
            native_plugin_context,
        };
        let mut store = Store::new(&self.engine, state);
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
        Ok(store)
    }

    fn spawn_epoch_tick_task(
        &self,
        epoch_tick_interval_ms: u64,
    ) -> (watch::Sender<bool>, JoinHandle<()>) {
        let tick_runtime = self.clone();
        let (tick_stop_tx, mut tick_stop_rx) = watch::channel(false);
        let tick_interval = Duration::from_millis(epoch_tick_interval_ms.max(1));
        let tick_task = tokio::spawn(async move {
            loop {
                if *tick_stop_rx.borrow() {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(tick_interval) => {
                        tick_runtime.increment_epoch();
                    }
                    changed = tick_stop_rx.changed() => {
                        if changed.is_err() || *tick_stop_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });
        (tick_stop_tx, tick_task)
    }

    /// Instantiates and runs a WASI CLI component asynchronously.
    ///
    /// Returns when execution completes or when shutdown is requested.
    async fn run_cli_component_async(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
        socket: Option<&RunnerSocketConfig>,
        native_plugin_context: NativePluginContext,
        plugin_dependencies: &[PluginDependency],
        capabilities: &CapabilityPolicy,
        mut shutdown: watch::Receiver<bool>,
        epoch_tick_interval_ms: u64,
    ) -> Result<(), ImagodError> {
        let component = Component::from_file(&self.engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;

        let mut linker = Linker::new(&self.engine);
        add_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI linker: {e}")))?;

        let mut store = self.build_store(args, envs, socket, native_plugin_context)?;
        let available_plugins = self
            .instantiate_plugin_dependencies(&mut store, plugin_dependencies)
            .await?;
        let explicit_dependency_names = all_dependency_names(plugin_dependencies);
        self.register_plugin_import_shims(
            &mut linker,
            &mut store,
            &component,
            "app",
            &explicit_dependency_names,
            capabilities,
            &available_plugins,
            None,
        )?;

        let run_future = async {
            let command = Command::instantiate_async(&mut store, &component, &linker)
                .await
                .map_err(|e| map_runtime_error(format!("component instantiate failed: {e}")))?;
            let run_result = command
                .wasi_cli_run()
                .call_run(&mut store)
                .await
                .map_err(|e| map_runtime_error(format!("wasi cli run trap: {e}")))?;

            run_result.map_err(|()| {
                ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_RUNTIME,
                    "wasi cli run returned failure status",
                )
            })
        };

        let (tick_stop_tx, tick_task) = self.spawn_epoch_tick_task(epoch_tick_interval_ms);

        let result = tokio::select! {
            _ = wait_for_shutdown(&mut shutdown) => Ok(()),
            result = run_future => result,
        };
        let _ = tick_stop_tx.send(true);
        let _ = tick_task.await;
        result
    }

    /// Instantiates a WASI HTTP incoming-handler and waits for shutdown.
    async fn run_http_component_async(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
        native_plugin_context: NativePluginContext,
        plugin_dependencies: &[PluginDependency],
        capabilities: &CapabilityPolicy,
        mut shutdown: watch::Receiver<bool>,
        epoch_tick_interval_ms: u64,
        mut http_ready_tx: Option<oneshot::Sender<()>>,
    ) -> Result<(), ImagodError> {
        let component = Component::from_file(&self.engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;

        let mut linker = Linker::new(&self.engine);
        add_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI linker: {e}")))?;
        add_only_http_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI HTTP linker: {e}")))?;

        let mut store = self.build_store(args, envs, None, native_plugin_context)?;
        let available_plugins = self
            .instantiate_plugin_dependencies(&mut store, plugin_dependencies)
            .await?;
        let explicit_dependency_names = all_dependency_names(plugin_dependencies);
        self.register_plugin_import_shims(
            &mut linker,
            &mut store,
            &component,
            "app",
            &explicit_dependency_names,
            capabilities,
            &available_plugins,
            None,
        )?;
        let proxy = Proxy::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(|e| map_runtime_error(format!("http component instantiate failed: {e}")))?;

        let (request_tx, request_rx) = mpsc::channel(HTTP_REQUEST_QUEUE_CAPACITY);
        let mut worker_task = Some(tokio::spawn(run_http_worker(store, proxy, request_rx)));
        let (tick_stop_tx, tick_task) = self.spawn_epoch_tick_task(epoch_tick_interval_ms);
        let already_running = {
            let mut guard = self.http_instance.lock().await;
            if guard.is_some() {
                true
            } else {
                *guard = Some(RunningHttpComponent {
                    request_tx,
                    worker_task: worker_task
                        .take()
                        .expect("worker task should exist before insertion"),
                });
                if let Some(ready_tx) = http_ready_tx.take() {
                    let _ = ready_tx.send(());
                }
                false
            }
        };
        if already_running {
            if let Some(worker_task) = worker_task.take() {
                worker_task.abort();
            }
            let _ = tick_stop_tx.send(true);
            let _ = tick_task.await;
            return Err(map_runtime_error(
                "http component is already running in this runtime instance".to_string(),
            ));
        }

        wait_for_shutdown(&mut shutdown).await;
        let running = {
            let mut guard = self.http_instance.lock().await;
            guard.take()
        };
        if let Some(running) = running {
            drop(running.request_tx);
            let _ = running.worker_task.await;
        }
        let _ = tick_stop_tx.send(true);
        let _ = tick_task.await;
        Ok(())
    }

    async fn handle_http_request_async(
        &self,
        request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, ImagodError> {
        let request_tx = {
            let guard = self.http_instance.lock().await;
            let running = guard.as_ref().ok_or_else(|| {
                ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_RUNTIME,
                    "http component is not running",
                )
            })?;
            running.request_tx.clone()
        };
        let (response_tx, response_rx) = oneshot::channel();
        request_tx
            .send(HttpWorkerRequest {
                request,
                response_tx,
            })
            .await
            .map_err(|_| {
                map_runtime_error("http component worker request channel is closed".to_string())
            })?;
        response_rx.await.map_err(|_| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNTIME,
                "http component worker did not return a response",
            )
        })?
    }

    async fn instantiate_plugin_dependencies(
        &self,
        store: &mut Store<WasiState>,
        dependencies: &[PluginDependency],
    ) -> Result<BTreeMap<String, AvailablePlugin>, ImagodError> {
        let mut loaded_components = BTreeMap::<String, Component>::new();
        for dep in dependencies {
            if dep.kind != PluginKind::Wasm {
                continue;
            }
            let component = dep.component.as_ref().ok_or_else(|| {
                map_runtime_error(format!(
                    "plugin dependency '{}' missing component definition",
                    dep.name
                ))
            })?;
            let loaded = Component::from_file(&self.engine, &component.path).map_err(|e| {
                map_runtime_error(format!(
                    "failed to load plugin component '{}' from {}: {e}",
                    dep.name,
                    component.path.display()
                ))
            })?;
            loaded_components.insert(dep.name.clone(), loaded);
        }
        let order = dependency_topological_order(dependencies, &loaded_components, &self.engine)?;
        let mut available = BTreeMap::<String, AvailablePlugin>::new();
        let explicit_dependency_names = all_dependency_names(dependencies);

        for dep_index in order {
            let dep = &dependencies[dep_index];
            match dep.kind {
                PluginKind::Native => {
                    if !self.native_plugins.has_plugin(&dep.name) {
                        return Err(map_runtime_error(format!(
                            "native plugin '{}' is declared in manifest but not registered in runtime",
                            dep.name
                        )));
                    }
                    available.insert(
                        dep.name.clone(),
                        AvailablePlugin {
                            kind: PluginKind::Native,
                            instance: None,
                        },
                    );
                }
                PluginKind::Wasm => {
                    let component = loaded_components.get(&dep.name).ok_or_else(|| {
                        map_runtime_error(format!(
                            "internal error: loaded component for dependency '{}' is missing",
                            dep.name
                        ))
                    })?;

                    let mut linker = Linker::new(&self.engine);
                    add_to_linker_async(&mut linker).map_err(|e| {
                        map_runtime_error(format!(
                            "failed to add WASI linker for plugin '{}': {e}",
                            dep.name
                        ))
                    })?;
                    add_only_http_to_linker_async(&mut linker).map_err(|e| {
                        map_runtime_error(format!(
                            "failed to add WASI HTTP linker for plugin '{}': {e}",
                            dep.name
                        ))
                    })?;
                    let self_instance = Arc::new(tokio::sync::Mutex::new(None));
                    self.register_plugin_import_shims(
                        &mut linker,
                        store,
                        component,
                        &dep.name,
                        &explicit_dependency_names,
                        &dep.capabilities,
                        &available,
                        Some(self_instance.clone()),
                    )?;

                    let instance = linker
                        .instantiate_async(&mut *store, component)
                        .await
                        .map_err(|e| {
                            map_runtime_error(format!(
                                "failed to instantiate plugin component '{}': {e}",
                                dep.name
                            ))
                        })?;
                    {
                        let mut guard = self_instance.lock().await;
                        *guard = Some(instance.clone());
                    }
                    available.insert(
                        dep.name.clone(),
                        AvailablePlugin {
                            kind: PluginKind::Wasm,
                            instance: Some(instance),
                        },
                    );
                }
            }
        }

        Ok(available)
    }

    fn register_plugin_import_shims(
        &self,
        linker: &mut Linker<WasiState>,
        store: &mut Store<WasiState>,
        component: &Component,
        caller_name: &str,
        explicit_dependency_names: &BTreeSet<String>,
        capabilities: &CapabilityPolicy,
        available_plugins: &BTreeMap<String, AvailablePlugin>,
        self_instance: Option<Arc<tokio::sync::Mutex<Option<wasmtime::component::Instance>>>>,
    ) -> Result<(), ImagodError> {
        let component_ty = component.component_type();
        let self_export_interfaces =
            collect_component_instance_export_names(component, &self.engine);
        let allow_self_provider = self_instance.is_some();
        let mut linked_native_imports = BTreeSet::<String>::new();

        for (import_name, import_item) in component_ty.imports(&self.engine) {
            let types::ComponentItem::ComponentInstance(instance_ty) = import_item else {
                continue;
            };
            if import_name.starts_with("wasi:") {
                enforce_wasi_import_capabilities(
                    caller_name,
                    capabilities,
                    import_name,
                    &instance_ty,
                    &self.engine,
                )?;
                continue;
            }
            let provider = resolve_import_provider(
                caller_name,
                import_name,
                &self_export_interfaces,
                explicit_dependency_names,
                available_plugins,
                allow_self_provider,
            )?;

            let native_dependency = match &provider {
                ImportProvider::Dependency(target_dependency) => {
                    available_plugins.get(target_dependency).and_then(|plugin| {
                        (plugin.kind == PluginKind::Native).then_some(target_dependency.as_str())
                    })
                }
                _ => None,
            };
            if let Some(target_dependency) = native_dependency {
                let native_plugin =
                    self.native_plugins
                        .plugin(target_dependency)
                        .ok_or_else(|| {
                            map_runtime_error(format!(
                                "native plugin '{}' is not registered in runtime registry",
                                target_dependency
                            ))
                        })?;
                if !native_plugin.supports_import(import_name) {
                    return Err(map_runtime_error(format!(
                        "native plugin '{}' does not support import '{}'",
                        target_dependency, import_name
                    )));
                }
                for (func_name, item) in instance_ty.exports(&self.engine) {
                    let types::ComponentItem::ComponentFunc(_) = item else {
                        continue;
                    };
                    if !is_dependency_function_allowed(
                        capabilities,
                        target_dependency,
                        import_name,
                        func_name,
                    ) {
                        return Err(map_runtime_unauthorized_error(format!(
                            "capability denied caller '{}' -> dependency '{}' function '{}.{}'",
                            caller_name, target_dependency, import_name, func_name
                        )));
                    }
                    let native_symbol = format!("{import_name}.{func_name}");
                    if !native_plugin.supports_symbol(&native_symbol) {
                        return Err(map_runtime_error(format!(
                            "native plugin '{}' does not expose symbol '{}'",
                            target_dependency, native_symbol
                        )));
                    }
                }
                if linked_native_imports.insert(import_name.to_string()) {
                    native_plugin.add_to_linker(linker)?;
                }
                continue;
            }

            let mut import_instance = linker.instance(import_name).map_err(|e| {
                map_runtime_error(format!(
                    "failed to define plugin import namespace '{}': {e}",
                    import_name
                ))
            })?;

            for (func_name, item) in instance_ty.exports(&self.engine) {
                let types::ComponentItem::ComponentFunc(import_ty) = item else {
                    continue;
                };
                match &provider {
                    ImportProvider::SelfComponent => {
                        let self_instance = self_instance.clone().ok_or_else(|| {
                            map_runtime_error(format!(
                                "self provider is unavailable for caller='{}', import='{}'",
                                caller_name, import_name
                            ))
                        })?;
                        let self_export_ty = resolve_component_export_type(
                            component,
                            &self.engine,
                            import_name,
                            func_name,
                        )?;
                        ensure_component_signatures_match(
                            &import_ty,
                            &self_export_ty,
                            import_name,
                            func_name,
                        )?;

                        let interface_name = import_name.to_string();
                        let function_name = func_name.to_string();
                        import_instance
                            .func_new_async(func_name, move |mut store, _ty, params, results| {
                                let self_instance = self_instance.clone();
                                let interface_name = interface_name.clone();
                                let function_name = function_name.clone();
                                Box::new(async move {
                                    let instance = {
                                        let guard = self_instance.lock().await;
                                        guard.as_ref().cloned()
                                    }
                                    .ok_or_else(|| {
                                        wasmtime::Error::msg(format!(
                                            "self provider instance is not ready for '{}.{}'",
                                            interface_name, function_name
                                        ))
                                    })?;
                                    let callee = resolve_wasm_export_from_instance(
                                        &mut store,
                                        &instance,
                                        &interface_name,
                                        &function_name,
                                    )
                                    .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
                                    callee.call_async(&mut store, params, results).await?;
                                    callee.post_return_async(&mut store).await?;
                                    Ok(())
                                })
                            })
                            .map_err(|e| {
                                map_runtime_error(format!(
                                    "failed to define self plugin shim '{}.{}': {e}",
                                    import_name, func_name
                                ))
                            })?;
                    }
                    ImportProvider::Dependency(target_dependency) => {
                        if !is_dependency_function_allowed(
                            capabilities,
                            target_dependency,
                            import_name,
                            func_name,
                        ) {
                            return Err(map_runtime_unauthorized_error(format!(
                                "capability denied caller '{}' -> dependency '{}' function '{}.{}'",
                                caller_name, target_dependency, import_name, func_name
                            )));
                        }

                        let plugin = available_plugins.get(target_dependency).ok_or_else(|| {
                            map_runtime_error(format!(
                                "missing target dependency '{}' for plugin import '{}.{}'",
                                target_dependency, import_name, func_name
                            ))
                        })?;
                        match plugin.kind {
                            PluginKind::Native => {
                                return Err(map_runtime_error(format!(
                                    "internal error: native plugin dependency '{}' should have been linked before fallback bridge '{}.{}'",
                                    target_dependency, import_name, func_name
                                )));
                            }
                            PluginKind::Wasm => {
                                let callee = resolve_wasm_plugin_export(
                                    &mut *store,
                                    plugin,
                                    import_name,
                                    func_name,
                                )?;
                                let callee_ty = callee.ty(&*store);
                                ensure_component_signatures_match(
                                    &import_ty,
                                    &callee_ty,
                                    import_name,
                                    func_name,
                                )?;

                                import_instance
                                    .func_new_async(
                                        func_name,
                                        move |mut store, _ty, params, results| {
                                            let callee = callee.clone();
                                            Box::new(async move {
                                                callee
                                                    .call_async(&mut store, params, results)
                                                    .await?;
                                                callee.post_return_async(&mut store).await?;
                                                Ok(())
                                            })
                                        },
                                    )
                                    .map_err(|e| {
                                        map_runtime_error(format!(
                                            "failed to define wasm plugin shim '{}.{}': {e}",
                                            import_name, func_name
                                        ))
                                    })?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
struct AvailablePlugin {
    kind: PluginKind,
    instance: Option<wasmtime::component::Instance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImportProvider {
    SelfComponent,
    Dependency(String),
}

async fn run_http_worker(
    mut store: Store<WasiState>,
    proxy: Proxy,
    mut request_rx: mpsc::Receiver<HttpWorkerRequest>,
) {
    while let Some(request) = request_rx.recv().await {
        let result = handle_http_request_in_store(&mut store, &proxy, request.request).await;
        let _ = request.response_tx.send(result);
    }
}

async fn handle_http_request_in_store(
    store: &mut Store<WasiState>,
    proxy: &Proxy,
    request: RuntimeHttpRequest,
) -> Result<RuntimeHttpResponse, ImagodError> {
    let request = runtime_request_to_hyper_request(request)?;
    let req = store
        .data_mut()
        .new_incoming_request(Scheme::Http, request)
        .map_err(|e| map_runtime_error(format!("failed to map incoming HTTP request: {e}")))?;

    let (sender, receiver) = oneshot::channel();
    let out = store
        .data_mut()
        .new_response_outparam(sender)
        .map_err(|e| map_runtime_error(format!("failed to allocate response outparam: {e}")))?;

    proxy
        .wasi_http_incoming_handler()
        .call_handle(store, req, out)
        .await
        .map_err(|e| map_runtime_error(format!("incoming-handler trap: {e}")))?;

    let response = receiver.await.map_err(|_| {
        map_runtime_error("incoming-handler did not set response outparam".to_string())
    })?;
    let response = response.map_err(|code| {
        map_runtime_error(format!(
            "incoming-handler returned wasi:http error: {code:?}"
        ))
    })?;

    runtime_response_from_hyper(response).await
}

fn configure_socket_policy(
    builder: &mut WasiCtxBuilder,
    socket: &RunnerSocketConfig,
) -> Result<(), ImagodError> {
    let listen_ip = socket.listen_addr.parse::<IpAddr>().map_err(|err| {
        map_runtime_error(format!(
            "invalid socket listen_addr '{}': {err}",
            socket.listen_addr
        ))
    })?;
    let listen_socket = SocketAddr::new(listen_ip, socket.listen_port);
    builder.allow_udp(socket.protocol.allows_udp());
    builder.allow_tcp(socket.protocol.allows_tcp());
    let direction = socket.direction;
    builder.socket_addr_check(move |address, use_kind| {
        let allowed = socket_addr_allowed(address, use_kind, listen_socket, direction);
        Box::pin(async move { allowed })
    });
    Ok(())
}

fn socket_addr_allowed(
    address: SocketAddr,
    use_kind: SocketAddrUse,
    listen_socket: SocketAddr,
    direction: RunnerSocketDirection,
) -> bool {
    match use_kind {
        SocketAddrUse::TcpBind | SocketAddrUse::UdpBind => {
            direction.allows_inbound() && address == listen_socket
        }
        SocketAddrUse::TcpConnect
        | SocketAddrUse::UdpConnect
        | SocketAddrUse::UdpOutgoingDatagram => direction.allows_outbound(),
    }
}

fn all_dependency_names(dependencies: &[PluginDependency]) -> BTreeSet<String> {
    dependencies.iter().map(|dep| dep.name.clone()).collect()
}

fn dependency_topological_order(
    dependencies: &[PluginDependency],
    loaded_components: &BTreeMap<String, Component>,
    engine: &Engine,
) -> Result<Vec<usize>, ImagodError> {
    let mut by_name = BTreeMap::<String, usize>::new();
    for (index, dep) in dependencies.iter().enumerate() {
        if by_name.insert(dep.name.clone(), index).is_some() {
            return Err(map_runtime_error(format!(
                "duplicate plugin dependency name '{}'",
                dep.name
            )));
        }
    }

    let mut indegree = vec![0usize; dependencies.len()];
    let mut edges = vec![BTreeSet::<usize>::new(); dependencies.len()];
    for (index, dep) in dependencies.iter().enumerate() {
        for req in &dep.requires {
            let req_index = by_name.get(req).ok_or_else(|| {
                map_runtime_error(format!(
                    "plugin dependency '{}' requires unknown dependency '{}'",
                    dep.name, req
                ))
            })?;
            add_dependency_edge(&mut edges, &mut indegree, *req_index, index);
        }

        let Some(component) = loaded_components.get(&dep.name) else {
            continue;
        };
        let self_export_interfaces = collect_component_instance_export_names(component, engine);
        let import_names = collect_component_instance_import_names(component, engine);
        let implicit_provider_indices = collect_implicit_provider_indices(
            &dep.name,
            &import_names,
            &self_export_interfaces,
            &by_name,
        );
        for provider_index in implicit_provider_indices {
            add_dependency_edge(&mut edges, &mut indegree, provider_index, index);
        }
    }

    let mut ready = dependencies
        .iter()
        .enumerate()
        .filter_map(|(idx, _)| (indegree[idx] == 0).then_some(idx))
        .collect::<Vec<_>>();
    ready.sort_unstable_by(|a, b| dependencies[*a].name.cmp(&dependencies[*b].name));

    let mut order = Vec::with_capacity(dependencies.len());
    while let Some(next) = ready.pop() {
        order.push(next);
        for edge in &edges[next] {
            indegree[*edge] = indegree[*edge].saturating_sub(1);
            if indegree[*edge] == 0 {
                ready.push(*edge);
            }
        }
        ready.sort_unstable_by(|a, b| dependencies[*a].name.cmp(&dependencies[*b].name));
    }

    if order.len() != dependencies.len() {
        return Err(map_runtime_error(
            "plugin dependency graph contains a cycle".to_string(),
        ));
    }
    Ok(order)
}

fn add_dependency_edge(
    edges: &mut [BTreeSet<usize>],
    indegree: &mut [usize],
    from: usize,
    to: usize,
) {
    if edges[from].insert(to) {
        indegree[to] += 1;
    }
}

fn resolve_import_provider(
    caller_name: &str,
    import_name: &str,
    self_export_interfaces: &BTreeSet<String>,
    explicit_dependency_names: &BTreeSet<String>,
    available_plugins: &BTreeMap<String, AvailablePlugin>,
    allow_self_provider: bool,
) -> Result<ImportProvider, ImagodError> {
    if allow_self_provider && self_export_interfaces.contains(import_name) {
        return Ok(ImportProvider::SelfComponent);
    }

    if let Some(explicit_dep) = parse_import_package_name(import_name)
        .filter(|name| explicit_dependency_names.contains(*name))
    {
        if available_plugins.contains_key(explicit_dep) {
            return Ok(ImportProvider::Dependency(explicit_dep.to_string()));
        }
        return Err(map_runtime_error(format!(
            "failed to resolve plugin import provider for caller='{}', import='{}': checked=self, explicit_dep='{}' exists but is not instantiated yet",
            caller_name, import_name, explicit_dep
        )));
    }

    let explicit_dep_label = parse_import_package_name(import_name).unwrap_or("<none>");
    Err(map_runtime_error(format!(
        "failed to resolve plugin import provider for caller='{}', import='{}': checked=self, explicit_dep='{}', result=not-found",
        caller_name, import_name, explicit_dep_label
    )))
}

fn collect_component_instance_export_names(
    component: &Component,
    engine: &Engine,
) -> BTreeSet<String> {
    component
        .component_type()
        .exports(engine)
        .filter_map(|(name, item)| match item {
            types::ComponentItem::ComponentInstance(_) => Some(name.to_string()),
            _ => None,
        })
        .collect()
}

fn collect_component_instance_import_names(component: &Component, engine: &Engine) -> Vec<String> {
    component
        .component_type()
        .imports(engine)
        .filter_map(|(name, item)| {
            if name.starts_with("wasi:") {
                return None;
            }
            match item {
                types::ComponentItem::ComponentInstance(_) => Some(name.to_string()),
                _ => None,
            }
        })
        .collect()
}

fn collect_implicit_provider_indices(
    dependency_name: &str,
    import_names: &[String],
    self_export_interfaces: &BTreeSet<String>,
    by_name: &BTreeMap<String, usize>,
) -> BTreeSet<usize> {
    let mut providers = BTreeSet::new();
    for import_name in import_names {
        if self_export_interfaces.contains(import_name) {
            continue;
        }
        let Some(package_name) = parse_import_package_name(import_name) else {
            continue;
        };
        if package_name == dependency_name {
            continue;
        }
        if let Some(index) = by_name.get(package_name) {
            providers.insert(*index);
        }
    }
    providers
}

fn parse_import_package_name(import_name: &str) -> Option<&str> {
    import_name
        .split_once('/')
        .map(|(package_name, _)| package_name)
}

fn is_dependency_function_allowed(
    policy: &CapabilityPolicy,
    dependency_name: &str,
    interface_name: &str,
    function_name: &str,
) -> bool {
    if policy.privileged {
        return true;
    }
    let Some(rules) = policy.deps.get(dependency_name) else {
        return false;
    };
    rules.iter().any(|rule| {
        rule == "*"
            || rule == function_name
            || rule == &format!("{interface_name}.{function_name}")
            || rule == &format!("{interface_name}/{function_name}")
            || rule == &format!("{interface_name}#{function_name}")
    })
}

fn enforce_wasi_import_capabilities(
    caller_name: &str,
    policy: &CapabilityPolicy,
    interface_name: &str,
    instance_ty: &types::ComponentInstance,
    engine: &Engine,
) -> Result<(), ImagodError> {
    for (function_name, item) in instance_ty.exports(engine) {
        let types::ComponentItem::ComponentFunc(_) = item else {
            continue;
        };
        ensure_wasi_function_allowed(caller_name, policy, interface_name, function_name)?;
    }
    Ok(())
}

fn ensure_wasi_function_allowed(
    caller_name: &str,
    policy: &CapabilityPolicy,
    interface_name: &str,
    function_name: &str,
) -> Result<(), ImagodError> {
    if is_wasi_function_allowed(policy, interface_name, function_name) {
        return Ok(());
    }
    Err(map_runtime_unauthorized_error(format!(
        "capability denied caller '{}' -> wasi '{}' function '{}'",
        caller_name, interface_name, function_name
    )))
}

fn is_wasi_function_allowed(
    policy: &CapabilityPolicy,
    interface_name: &str,
    function_name: &str,
) -> bool {
    if policy.privileged {
        return true;
    }
    let Some(rules) = policy.wasi.get(interface_name) else {
        return false;
    };
    rules.iter().any(|rule| {
        rule == "*"
            || rule == function_name
            || rule == &format!("{interface_name}.{function_name}")
            || rule == &format!("{interface_name}/{function_name}")
            || rule == &format!("{interface_name}#{function_name}")
    })
}

fn ensure_component_signatures_match(
    import_ty: &types::ComponentFunc,
    callee_ty: &types::ComponentFunc,
    interface_name: &str,
    function_name: &str,
) -> Result<(), ImagodError> {
    let import_params = import_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    let callee_params = callee_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    if import_params != callee_params {
        return Err(map_runtime_error(format!(
            "plugin import type mismatch for '{}.{}': parameter types differ",
            interface_name, function_name
        )));
    }

    let import_results = import_ty.results().collect::<Vec<_>>();
    let callee_results = callee_ty.results().collect::<Vec<_>>();
    if import_results != callee_results {
        return Err(map_runtime_error(format!(
            "plugin import type mismatch for '{}.{}': result types differ",
            interface_name, function_name
        )));
    }

    Ok(())
}

fn resolve_component_export_type(
    component: &Component,
    engine: &Engine,
    interface_name: &str,
    function_name: &str,
) -> Result<types::ComponentFunc, ImagodError> {
    let component_ty = component.component_type();
    let interface_ty = component_ty
        .exports(engine)
        .find_map(|(export_name, export_item)| {
            if export_name != interface_name {
                return None;
            }
            match export_item {
                types::ComponentItem::ComponentInstance(instance_ty) => Some(instance_ty),
                _ => None,
            }
        })
        .ok_or_else(|| {
            map_runtime_error(format!(
                "self provider export interface '{}' was not found",
                interface_name
            ))
        })?;

    interface_ty
        .exports(engine)
        .find_map(|(export_name, export_item)| {
            if export_name != function_name {
                return None;
            }
            match export_item {
                types::ComponentItem::ComponentFunc(func_ty) => Some(func_ty),
                _ => None,
            }
        })
        .ok_or_else(|| {
            map_runtime_error(format!(
                "self provider export function '{}.{}' was not found",
                interface_name, function_name
            ))
        })
}

fn resolve_wasm_plugin_export(
    store: impl wasmtime::AsContextMut<Data = WasiState>,
    plugin: &AvailablePlugin,
    interface_name: &str,
    function_name: &str,
) -> Result<Func, ImagodError> {
    let instance = plugin.instance.as_ref().ok_or_else(|| {
        map_runtime_error("internal error: wasm plugin dependency is missing instance".to_string())
    })?;
    resolve_wasm_export_from_instance(store, instance, interface_name, function_name)
}

fn resolve_wasm_export_from_instance(
    mut store: impl wasmtime::AsContextMut<Data = WasiState>,
    instance: &wasmtime::component::Instance,
    interface_name: &str,
    function_name: &str,
) -> Result<Func, ImagodError> {
    let interface_index = instance
        .get_export_index(store.as_context_mut(), None, interface_name)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "plugin export interface '{}' was not found",
                interface_name
            ))
        })?;
    let function_index = instance
        .get_export_index(
            store.as_context_mut(),
            Some(&interface_index),
            function_name,
        )
        .or_else(|| instance.get_export_index(store.as_context_mut(), None, function_name))
        .ok_or_else(|| {
            map_runtime_error(format!(
                "plugin export function '{}.{}' was not found",
                interface_name, function_name
            ))
        })?;
    instance
        .get_func(store.as_context_mut(), function_index)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "plugin export '{}.{}' is not a function",
                interface_name, function_name
            ))
        })
}

impl ComponentRuntime for WasmRuntime {
    fn validate_component(&self, component_path: &Path) -> Result<(), ImagodError> {
        self.validate_component_loadable(component_path)
    }

    fn run_component<'a>(&'a self, request: RuntimeRunRequest) -> RuntimeRunFuture<'a> {
        Box::pin(async move {
            let RuntimeRunRequest {
                app_type,
                runner_id,
                service_name,
                release_hash,
                component_path,
                args,
                envs,
                socket,
                plugin_dependencies,
                capabilities,
                shutdown,
                epoch_tick_interval_ms,
                http_ready_tx,
            } = request;
            let native_plugin_context =
                NativePluginContext::new(service_name, release_hash, runner_id, app_type);

            match app_type {
                RunnerAppType::Cli => {
                    if socket.is_some() {
                        return Err(map_runtime_error(
                            "socket config is only allowed when app_type=socket".to_string(),
                        ));
                    }
                    self.run_cli_component_async(
                        &component_path,
                        &args,
                        &envs,
                        None,
                        native_plugin_context.clone(),
                        &plugin_dependencies,
                        &capabilities,
                        shutdown,
                        epoch_tick_interval_ms,
                    )
                    .await
                }
                RunnerAppType::Http => {
                    if socket.is_some() {
                        return Err(map_runtime_error(
                            "socket config is only allowed when app_type=socket".to_string(),
                        ));
                    }
                    self.run_http_component_async(
                        &component_path,
                        &args,
                        &envs,
                        native_plugin_context.clone(),
                        &plugin_dependencies,
                        &capabilities,
                        shutdown,
                        epoch_tick_interval_ms,
                        http_ready_tx,
                    )
                    .await
                }
                RunnerAppType::Socket => {
                    let socket = socket.as_ref().ok_or_else(|| {
                        map_runtime_error(
                            "app_type=socket requires socket runtime settings".to_string(),
                        )
                    })?;
                    self.run_cli_component_async(
                        &component_path,
                        &args,
                        &envs,
                        Some(socket),
                        native_plugin_context,
                        &plugin_dependencies,
                        &capabilities,
                        shutdown,
                        epoch_tick_interval_ms,
                    )
                    .await
                }
            }
        })
    }

    fn handle_http_request<'a>(&'a self, request: RuntimeHttpRequest) -> RuntimeHttpFuture<'a> {
        Box::pin(async move { self.handle_http_request_async(request).await })
    }
}

fn runtime_request_to_hyper_request(
    request: RuntimeHttpRequest,
) -> Result<hyper::Request<BoxBody<Bytes, hyper::Error>>, ImagodError> {
    let method = hyper::Method::from_bytes(request.method.as_bytes()).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNTIME,
            format!("invalid http method '{}': {e}", request.method),
        )
    })?;

    let uri_text = if request.uri.is_empty() {
        "/".to_string()
    } else {
        request.uri
    };
    let uri = uri_text.parse::<hyper::Uri>().map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNTIME,
            format!("invalid http uri '{uri_text}': {e}"),
        )
    })?;

    let mut builder = hyper::Request::builder().method(method).uri(uri);
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in request.headers {
            let name = hyper::header::HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_RUNTIME,
                    format!("invalid header name '{name}': {e}"),
                )
            })?;
            let value = hyper::header::HeaderValue::from_bytes(&value).map_err(|e| {
                ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_RUNTIME,
                    format!("invalid header value for '{name}': {e}"),
                )
            })?;
            headers.append(name, value);
        }
    }

    let body = BoxBody::new(
        Full::new(Bytes::from(request.body))
            .map_err(|never| match never {})
            .boxed(),
    );
    builder.body(body).map_err(|e| {
        map_runtime_error(format!(
            "failed to build hyper request for incoming-handler: {e}"
        ))
    })
}

async fn runtime_response_from_hyper(
    response: hyper::Response<wasmtime_wasi_http::body::HyperOutgoingBody>,
) -> Result<RuntimeHttpResponse, ImagodError> {
    let (parts, body) = response.into_parts();
    let collected = BodyExt::collect(body)
        .await
        .map_err(|e| map_runtime_error(format!("failed to collect outgoing response body: {e}")))?;
    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();

    Ok(RuntimeHttpResponse {
        status: parts.status.as_u16(),
        headers,
        body: collected.to_bytes().to_vec(),
    })
}

/// Maps runtime-originated failures to a unified internal error shape.
fn map_runtime_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_RUNTIME, message)
}

fn map_runtime_unauthorized_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Unauthorized, STAGE_RUNTIME, message)
}

/// Waits until shutdown flag is set or sender side is dropped.
async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_ipc::{
        RunnerAppType, RunnerSocketConfig, RunnerSocketDirection, RunnerSocketProtocol,
    };
    use imagod_runtime_internal::{RuntimeHttpRequest, RuntimeRunRequest};
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::SocketAddr;
    use std::path::PathBuf;

    fn sample_socket_config() -> RunnerSocketConfig {
        RunnerSocketConfig {
            protocol: RunnerSocketProtocol::Udp,
            direction: RunnerSocketDirection::Inbound,
            listen_addr: "0.0.0.0".to_string(),
            listen_port: 514,
        }
    }

    #[test]
    fn resolve_import_provider_prefers_self_component() {
        let self_exports = BTreeSet::from(["chikoski:name/name-provider".to_string()]);
        let explicit_names = BTreeSet::from(["chikoski:name".to_string()]);
        let available_plugins = BTreeMap::from([(
            "chikoski:name".to_string(),
            AvailablePlugin {
                kind: PluginKind::Native,
                instance: None,
            },
        )]);

        let provider = resolve_import_provider(
            "chikoski:hello",
            "chikoski:name/name-provider",
            &self_exports,
            &explicit_names,
            &available_plugins,
            true,
        )
        .expect("self provider should resolve");
        assert_eq!(provider, ImportProvider::SelfComponent);
    }

    #[test]
    fn resolve_import_provider_falls_back_to_explicit_dependency() {
        let self_exports = BTreeSet::new();
        let explicit_names = BTreeSet::from(["chikoski:name".to_string()]);
        let available_plugins = BTreeMap::from([(
            "chikoski:name".to_string(),
            AvailablePlugin {
                kind: PluginKind::Native,
                instance: None,
            },
        )]);

        let provider = resolve_import_provider(
            "chikoski:hello",
            "chikoski:name/name-provider",
            &self_exports,
            &explicit_names,
            &available_plugins,
            true,
        )
        .expect("explicit dependency fallback should resolve");
        assert_eq!(
            provider,
            ImportProvider::Dependency("chikoski:name".to_string())
        );
    }

    #[test]
    fn resolve_import_provider_reports_unresolved_import() {
        let err = resolve_import_provider(
            "chikoski:hello",
            "chikoski:name/name-provider",
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            true,
        )
        .expect_err("missing provider should fail");
        assert!(
            err.message.contains("caller='chikoski:hello'")
                && err.message.contains("import='chikoski:name/name-provider'")
                && err.message.contains("checked=self")
                && err.message.contains("result=not-found"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn collect_implicit_provider_indices_uses_explicit_dep_when_self_missing() {
        let by_name = BTreeMap::from([
            ("chikoski:hello".to_string(), 0usize),
            ("chikoski:name".to_string(), 1usize),
        ]);
        let import_names = vec!["chikoski:name/name-provider".to_string()];
        let indices = collect_implicit_provider_indices(
            "chikoski:hello",
            &import_names,
            &BTreeSet::new(),
            &by_name,
        );
        assert_eq!(indices, BTreeSet::from([1usize]));
    }

    #[test]
    fn collect_implicit_provider_indices_skips_when_self_exports_interface() {
        let by_name = BTreeMap::from([
            ("chikoski:hello".to_string(), 0usize),
            ("chikoski:name".to_string(), 1usize),
        ]);
        let import_names = vec!["chikoski:name/name-provider".to_string()];
        let self_exports = BTreeSet::from(["chikoski:name/name-provider".to_string()]);
        let indices = collect_implicit_provider_indices(
            "chikoski:hello",
            &import_names,
            &self_exports,
            &by_name,
        );
        assert!(
            indices.is_empty(),
            "self export should suppress implicit edge"
        );
    }

    #[test]
    fn wasi_capability_denies_when_policy_is_empty() {
        let allowed = is_wasi_function_allowed(
            &CapabilityPolicy::default(),
            "wasi:cli/environment",
            "get-environment",
        );
        assert!(!allowed, "empty policy should deny wasi function");
    }

    #[test]
    fn wasi_capability_allows_when_privileged() {
        let policy = CapabilityPolicy {
            privileged: true,
            deps: BTreeMap::new(),
            wasi: BTreeMap::new(),
        };
        let allowed = is_wasi_function_allowed(&policy, "wasi:cli/environment", "get-environment");
        assert!(allowed, "privileged policy should allow all wasi calls");
    }

    #[test]
    fn wasi_capability_allows_when_rule_is_wildcard() {
        let policy = CapabilityPolicy {
            privileged: false,
            deps: BTreeMap::new(),
            wasi: BTreeMap::from([("wasi:cli/environment".to_string(), vec!["*".to_string()])]),
        };
        let allowed = is_wasi_function_allowed(&policy, "wasi:cli/environment", "get-environment");
        assert!(allowed, "wildcard rule should allow wasi function");
    }

    #[test]
    fn wasi_capability_rejects_unlisted_function() {
        let policy = CapabilityPolicy {
            privileged: false,
            deps: BTreeMap::new(),
            wasi: BTreeMap::from([(
                "wasi:cli/environment".to_string(),
                vec!["get-arguments".to_string()],
            )]),
        };
        let allowed = is_wasi_function_allowed(&policy, "wasi:cli/environment", "get-environment");
        assert!(!allowed, "unlisted function should be denied");
    }

    #[test]
    fn wasi_capability_denial_maps_to_unauthorized() {
        let err = ensure_wasi_function_allowed(
            "app",
            &CapabilityPolicy::default(),
            "wasi:cli/environment",
            "get-environment",
        )
        .expect_err("empty policy should deny wasi function");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert!(
            err.message.contains("capability denied caller 'app'"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn native_plugin_app_type_text_is_stable() {
        assert_eq!(app_type_text(RunnerAppType::Cli), "cli");
        assert_eq!(app_type_text(RunnerAppType::Http), "http");
        assert_eq!(app_type_text(RunnerAppType::Socket), "socket");
    }

    #[test]
    fn native_plugin_context_stores_runner_metadata() {
        let context = NativePluginContext::new(
            "svc-test".to_string(),
            "release-test".to_string(),
            "runner-test".to_string(),
            RunnerAppType::Http,
        );
        assert_eq!(context.service_name(), "svc-test");
        assert_eq!(context.release_hash(), "release-test");
        assert_eq!(context.runner_id(), "runner-test");
        assert_eq!(context.app_type(), "http");
    }

    #[tokio::test]
    async fn socket_type_requires_socket_config() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Socket,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: PathBuf::from("/tmp/unused.wasm"),
                args: Vec::new(),
                envs: BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_ready_tx: None,
            })
            .await
            .expect_err("socket type should require socket config");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("requires socket runtime settings"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn socket_type_uses_cli_execution_branch_when_socket_config_exists() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Socket,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: PathBuf::from("/tmp/non-existent-socket-component.wasm"),
                args: Vec::new(),
                envs: BTreeMap::new(),
                socket: Some(sample_socket_config()),
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_ready_tx: None,
            })
            .await
            .expect_err("missing component path should fail");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("failed to load component"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn http_type_uses_http_execution_branch() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Http,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: PathBuf::from("/tmp/non-existent-http-component.wasm"),
                args: Vec::new(),
                envs: BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_ready_tx: None,
            })
            .await
            .expect_err("missing component path should fail");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("failed to load component"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn handle_http_request_requires_running_http_component() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let err = runtime
            .handle_http_request(RuntimeHttpRequest {
                method: "GET".to_string(),
                uri: "/".to_string(),
                headers: Vec::new(),
                body: Vec::new(),
            })
            .await
            .expect_err("request should fail when no http component is running");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.message.contains("not running"));
    }

    #[test]
    fn socket_addr_allowed_restricts_inbound_to_exact_listen_endpoint() {
        let listen = "0.0.0.0:514"
            .parse::<SocketAddr>()
            .expect("listen socket should parse");
        let allowed = socket_addr_allowed(
            listen,
            SocketAddrUse::UdpBind,
            listen,
            RunnerSocketDirection::Inbound,
        );
        let denied_different_port = socket_addr_allowed(
            "0.0.0.0:515"
                .parse::<SocketAddr>()
                .expect("socket should parse"),
            SocketAddrUse::UdpBind,
            listen,
            RunnerSocketDirection::Inbound,
        );
        let denied_outbound_only = socket_addr_allowed(
            listen,
            SocketAddrUse::UdpBind,
            listen,
            RunnerSocketDirection::Outbound,
        );
        assert!(allowed);
        assert!(!denied_different_port);
        assert!(!denied_outbound_only);
    }

    #[test]
    fn socket_addr_allowed_gates_outbound_by_direction_only() {
        let listen = "0.0.0.0:514"
            .parse::<SocketAddr>()
            .expect("listen socket should parse");
        let remote = "192.0.2.10:1234"
            .parse::<SocketAddr>()
            .expect("remote socket should parse");
        let outbound_allowed = socket_addr_allowed(
            remote,
            SocketAddrUse::UdpOutgoingDatagram,
            listen,
            RunnerSocketDirection::Both,
        );
        let outbound_denied = socket_addr_allowed(
            remote,
            SocketAddrUse::TcpConnect,
            listen,
            RunnerSocketDirection::Inbound,
        );
        assert!(outbound_allowed);
        assert!(!outbound_denied);
    }
}
