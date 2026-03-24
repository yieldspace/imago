use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use imago_protocol::ErrorCode;
use imagod_common::{
    DEFAULT_WASM_GUARD_BEFORE_LINEAR_MEMORY, DEFAULT_WASM_MEMORY_GUARD_SIZE_BYTES,
    DEFAULT_WASM_MEMORY_RESERVATION_BYTES, DEFAULT_WASM_MEMORY_RESERVATION_FOR_GROWTH_BYTES,
    DEFAULT_WASM_PARALLEL_COMPILATION, ImagodError,
};
use imagod_ipc::{
    CapabilityPolicy, PluginDependency, RunnerAppType, RunnerSocketConfig, RunnerSocketDirection,
    RunnerWasiMount, ServiceBinding, WasiHttpOutboundRule,
};
#[cfg(test)]
use imagod_runtime_internal::RuntimeInvokeContext;
use imagod_runtime_internal::{
    ComponentRuntime, HttpComponentSupervisor, PluginResolver, RuntimeHttpRequest,
    RuntimeHttpResponse, RuntimeHttpWorkItem, RuntimeInvokeRequest, RuntimeInvoker,
    RuntimeRunRequest,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Func, Linker},
};
use wasmtime_wasi::{
    DirPerms, FilePerms, WasiCtxBuilder, p2::add_to_linker_async, p2::bindings::Command,
    sockets::SocketAddrUse,
};
use wasmtime_wasi_http::p2::{add_only_http_to_linker_async, bindings::Proxy};

use crate::{
    HTTP_REQUEST_QUEUE_CAPACITY, NativePluginContext, STAGE_RUNTIME, WasiState, app_type_text,
    capability_checker::DefaultCapabilityChecker,
    http_supervisor::{DefaultHttpComponentSupervisor, run_http_worker},
    map_runtime_error,
    native_plugins::NativePluginRegistry,
    plugin_resolver::{
        DefaultPluginResolver, instantiate_plugin_dependencies, register_plugin_import_shims,
    },
    rpc_values::{decode_payload_values, encode_payload_values, placeholder_values},
    wasi_nn,
};

/// Wasmtime engine-level memory tuning knobs propagated from manager runtime config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmEngineTuning {
    /// Wasmtime linear-memory reservation size in bytes.
    pub memory_reservation_bytes: u64,
    /// Wasmtime extra reservation size for linear-memory growth in bytes.
    pub memory_reservation_for_growth_bytes: u64,
    /// Wasmtime linear-memory guard size in bytes.
    pub memory_guard_size_bytes: u64,
    /// Whether Wasmtime reserves a guard region before linear memory.
    pub guard_before_linear_memory: bool,
    /// Whether Wasmtime compiles modules in parallel.
    pub parallel_compilation: bool,
}

impl Default for WasmEngineTuning {
    fn default() -> Self {
        Self {
            memory_reservation_bytes: DEFAULT_WASM_MEMORY_RESERVATION_BYTES,
            memory_reservation_for_growth_bytes: DEFAULT_WASM_MEMORY_RESERVATION_FOR_GROWTH_BYTES,
            memory_guard_size_bytes: DEFAULT_WASM_MEMORY_GUARD_SIZE_BYTES,
            guard_before_linear_memory: DEFAULT_WASM_GUARD_BEFORE_LINEAR_MEMORY,
            parallel_compilation: DEFAULT_WASM_PARALLEL_COMPILATION,
        }
    }
}

impl WasmEngineTuning {
    fn apply_to_config(self, config: &mut Config) {
        config.memory_reservation(self.memory_reservation_bytes);
        config.memory_reservation_for_growth(self.memory_reservation_for_growth_bytes);
        config.memory_guard_size(self.memory_guard_size_bytes);
        config.guard_before_linear_memory(self.guard_before_linear_memory);
        config.parallel_compilation(self.parallel_compilation);
    }
}

/// Runner-local wrapper around a configured Wasmtime engine.
pub struct WasmRuntime {
    engine: Arc<Engine>,
    component_cache: Arc<std::sync::RwLock<BTreeMap<PathBuf, Arc<Component>>>>,
    native_plugins: NativePluginRegistry,
    plugin_resolver: Arc<DefaultPluginResolver>,
    http_supervisor: Arc<DefaultHttpComponentSupervisor>,
    capability_checker: Arc<DefaultCapabilityChecker>,
}

impl Clone for WasmRuntime {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            component_cache: self.component_cache.clone(),
            native_plugins: self.native_plugins.clone(),
            plugin_resolver: self.plugin_resolver.clone(),
            http_supervisor: self.http_supervisor.clone(),
            capability_checker: self.capability_checker.clone(),
        }
    }
}

impl WasmRuntime {
    /// Creates a runtime with component model, async support, and epoch interruption enabled.
    pub fn new() -> Result<Self, ImagodError> {
        Self::new_with_native_plugins_and_tuning(
            NativePluginRegistry::default(),
            WasmEngineTuning::default(),
        )
    }

    /// Creates a runtime with explicit Wasmtime engine memory tuning.
    pub fn new_with_tuning(tuning: WasmEngineTuning) -> Result<Self, ImagodError> {
        Self::new_with_native_plugins_and_tuning(NativePluginRegistry::default(), tuning)
    }

    /// Creates a runtime with a native plugin registry injected by manager build.
    pub fn new_with_native_plugins(
        native_plugins: NativePluginRegistry,
    ) -> Result<Self, ImagodError> {
        Self::new_with_native_plugins_and_tuning(native_plugins, WasmEngineTuning::default())
    }

    /// Creates a runtime with native plugins and explicit Wasmtime engine memory tuning.
    pub fn new_with_native_plugins_and_tuning(
        native_plugins: NativePluginRegistry,
        tuning: WasmEngineTuning,
    ) -> Result<Self, ImagodError> {
        Self::new_with_runtime_contracts(
            native_plugins,
            Arc::new(DefaultPluginResolver),
            Arc::new(DefaultHttpComponentSupervisor::new()),
            Arc::new(DefaultCapabilityChecker),
            tuning,
        )
    }

    fn new_with_runtime_contracts(
        native_plugins: NativePluginRegistry,
        plugin_resolver: Arc<DefaultPluginResolver>,
        http_supervisor: Arc<DefaultHttpComponentSupervisor>,
        capability_checker: Arc<DefaultCapabilityChecker>,
        tuning: WasmEngineTuning,
    ) -> Result<Self, ImagodError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        tuning.apply_to_config(&mut config);

        let engine = Engine::new(&config)
            .map_err(|e| map_runtime_error(format!("engine init failed: {e}")))?;

        Ok(Self {
            engine: Arc::new(engine),
            component_cache: Arc::new(std::sync::RwLock::new(BTreeMap::new())),
            native_plugins,
            plugin_resolver,
            http_supervisor,
            capability_checker,
        })
    }

    /// Increments the engine epoch to unblock interruption-aware execution.
    pub fn increment_epoch(&self) {
        self.engine.increment_epoch();
    }

    fn validate_component_loadable(&self, component_path: &Path) -> Result<(), ImagodError> {
        let _ = self.load_component_cached(component_path)?;
        Ok(())
    }

    fn load_component_cached(&self, component_path: &Path) -> Result<Arc<Component>, ImagodError> {
        if let Some(component) = self
            .component_cache
            .read()
            .map_err(|_| map_runtime_error("component cache lock is poisoned".to_string()))?
            .get(component_path)
            .cloned()
        {
            return Ok(component);
        }

        let mut cache = self
            .component_cache
            .write()
            .map_err(|_| map_runtime_error("component cache lock is poisoned".to_string()))?;
        if let Some(existing) = cache.get(component_path).cloned() {
            return Ok(existing);
        }
        let loaded = Arc::new(
            Component::from_file(&self.engine, component_path).map_err(|e| {
                map_runtime_error(format!(
                    "failed to load component {}: {e}",
                    component_path.display()
                ))
            })?,
        );
        cache.insert(component_path.to_path_buf(), loaded.clone());
        Ok(loaded)
    }

    fn build_store(
        &self,
        args: &[String],
        envs: &BTreeMap<String, String>,
        wasi_mounts: &[RunnerWasiMount],
        wasi_http_outbound: &[WasiHttpOutboundRule],
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
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>();
            builder.envs(&vars);
        }
        if !wasi_mounts.is_empty() {
            configure_wasi_mounts(&mut builder, wasi_mounts)?;
        }
        if let Some(socket) = socket {
            configure_socket_policy(&mut builder, socket)?;
        }

        let state = WasiState::new(
            builder.build(),
            wasmtime_wasi_http::WasiHttpCtx::new(),
            wasi_http_outbound.to_vec(),
            native_plugin_context,
        );
        let mut store = Store::new(&self.engine, state);
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
        Ok(store)
    }

    fn spawn_epoch_tick_task(
        &self,
        epoch_tick_interval_ms: u64,
    ) -> (watch::Sender<bool>, JoinHandle<()>) {
        let tick_runtime = (*self).clone();
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

    fn build_component_linker(&self) -> Result<Linker<WasiState>, ImagodError> {
        let mut linker = Linker::new(&self.engine);
        add_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI linker: {e}")))?;
        add_only_http_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI HTTP linker: {e}")))?;
        wasi_nn::add_to_linker(&mut linker)?;
        Ok(linker)
    }

    fn component_requires_wasi_nn(&self, component: &Component) -> bool {
        component
            .component_type()
            .imports(&self.engine)
            .any(|(name, _)| name.starts_with("wasi:nn/"))
    }

    fn ensure_wasi_nn_component_support(&self, component: &Component) -> Result<(), ImagodError> {
        if !self.component_requires_wasi_nn(component) {
            return Ok(());
        }

        if !wasi_nn::has_enabled_feature() {
            return Err(map_runtime_error(
                "wasi-nn backend is not enabled; rebuild imagod with feature 'wasi-nn-cvitek', 'wasi-nn-openvino', or 'wasi-nn-onnx'"
                    .to_string(),
            ));
        }

        let available_backends = wasi_nn::available_backend_names();
        if available_backends.is_empty() {
            return Err(map_runtime_error(format!(
                "wasi-nn backend is enabled but no backends are available on this target (enabled features: {})",
                wasi_nn::enabled_feature_names().join(", ")
            )));
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_cli_component_async(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
        wasi_mounts: &[RunnerWasiMount],
        wasi_http_outbound: &[WasiHttpOutboundRule],
        socket: Option<&RunnerSocketConfig>,
        native_plugin_context: NativePluginContext,
        plugin_dependencies: &[PluginDependency],
        capabilities: &CapabilityPolicy,
        bindings: &[ServiceBinding],
        mut shutdown: watch::Receiver<bool>,
        epoch_tick_interval_ms: u64,
    ) -> Result<(), ImagodError> {
        let component = self.load_component_cached(component_path)?;

        let mut linker = self.build_component_linker()?;

        let mut store = self.build_store(
            args,
            envs,
            wasi_mounts,
            wasi_http_outbound,
            socket,
            native_plugin_context,
        )?;
        let available_plugins = instantiate_plugin_dependencies(
            self.plugin_resolver.as_ref(),
            self.capability_checker.as_ref(),
            &self.native_plugins,
            &self.engine,
            &mut store,
            plugin_dependencies,
        )
        .await?;
        let explicit_dependency_names = self
            .plugin_resolver
            .all_dependency_names(plugin_dependencies);
        register_plugin_import_shims(
            self.plugin_resolver.as_ref(),
            self.capability_checker.as_ref(),
            &self.native_plugins,
            &self.engine,
            &mut linker,
            &mut store,
            component.as_ref(),
            "app",
            &explicit_dependency_names,
            capabilities,
            bindings,
            &available_plugins,
            None,
        )?;
        self.ensure_wasi_nn_component_support(component.as_ref())?;

        let run_future = async {
            let command = Command::instantiate_async(&mut store, component.as_ref(), &linker)
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

    #[allow(clippy::too_many_arguments)]
    async fn run_http_component_async(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
        wasi_mounts: &[RunnerWasiMount],
        wasi_http_outbound: &[WasiHttpOutboundRule],
        native_plugin_context: NativePluginContext,
        plugin_dependencies: &[PluginDependency],
        capabilities: &CapabilityPolicy,
        bindings: &[ServiceBinding],
        mut shutdown: watch::Receiver<bool>,
        epoch_tick_interval_ms: u64,
        http_worker_count: u32,
        http_worker_queue_capacity: u32,
        http_ready_tx: Option<oneshot::Sender<()>>,
    ) -> Result<(), ImagodError> {
        let component = self.load_component_cached(component_path)?;

        let mut linker = self.build_component_linker()?;

        let mut store = self.build_store(
            args,
            envs,
            wasi_mounts,
            wasi_http_outbound,
            None,
            native_plugin_context,
        )?;
        let available_plugins = instantiate_plugin_dependencies(
            self.plugin_resolver.as_ref(),
            self.capability_checker.as_ref(),
            &self.native_plugins,
            &self.engine,
            &mut store,
            plugin_dependencies,
        )
        .await?;
        let explicit_dependency_names = self
            .plugin_resolver
            .all_dependency_names(plugin_dependencies);
        register_plugin_import_shims(
            self.plugin_resolver.as_ref(),
            self.capability_checker.as_ref(),
            &self.native_plugins,
            &self.engine,
            &mut linker,
            &mut store,
            component.as_ref(),
            "app",
            &explicit_dependency_names,
            capabilities,
            bindings,
            &available_plugins,
            None,
        )?;
        self.ensure_wasi_nn_component_support(component.as_ref())?;

        let proxy = Proxy::instantiate_async(&mut store, component.as_ref(), &linker)
            .await
            .map_err(|e| map_runtime_error(format!("http component instantiate failed: {e}")))?;

        let request_queue_capacity =
            http_request_queue_capacity(http_worker_count, http_worker_queue_capacity);
        let (request_tx, request_rx) = mpsc::channel(request_queue_capacity);
        let worker_task = tokio::spawn(run_http_worker(store, proxy, request_rx));
        let (tick_stop_tx, tick_task) = self.spawn_epoch_tick_task(epoch_tick_interval_ms);

        if let Err(err) = self
            .http_supervisor
            .register_http_component(request_tx, http_ready_tx)
            .await
        {
            worker_task.abort();
            let _ = tick_stop_tx.send(true);
            let _ = tick_task.await;
            return Err(err);
        }

        wait_for_shutdown(&mut shutdown).await;
        let request_tx = self.http_supervisor.unregister_http_component().await;
        drop(request_tx);
        let _ = worker_task.await;
        let _ = tick_stop_tx.send(true);
        let _ = tick_task.await;
        Ok(())
    }

    async fn handle_http_request_async(
        &self,
        request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, ImagodError> {
        let request_tx = self.http_supervisor.request_sender().await?;
        let response_rx = dispatch_http_work_item(&request_tx, request)?;
        response_rx.await.map_err(|_| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNTIME,
                "http component worker did not return a response",
            )
        })?
    }

    async fn run_rpc_component_async(
        &self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), ImagodError> {
        wait_for_shutdown(&mut shutdown).await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn invoke_rpc_component_async(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
        wasi_mounts: &[RunnerWasiMount],
        wasi_http_outbound: &[WasiHttpOutboundRule],
        native_plugin_context: NativePluginContext,
        plugin_dependencies: &[PluginDependency],
        capabilities: &CapabilityPolicy,
        bindings: &[ServiceBinding],
        interface_id: &str,
        function: &str,
        payload_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, ImagodError> {
        let component = self.load_component_cached(component_path)?;

        let mut linker = self.build_component_linker()?;

        let mut store = self.build_store(
            args,
            envs,
            wasi_mounts,
            wasi_http_outbound,
            None,
            native_plugin_context,
        )?;
        let available_plugins = instantiate_plugin_dependencies(
            self.plugin_resolver.as_ref(),
            self.capability_checker.as_ref(),
            &self.native_plugins,
            &self.engine,
            &mut store,
            plugin_dependencies,
        )
        .await?;
        let explicit_dependency_names = self
            .plugin_resolver
            .all_dependency_names(plugin_dependencies);
        register_plugin_import_shims(
            self.plugin_resolver.as_ref(),
            self.capability_checker.as_ref(),
            &self.native_plugins,
            &self.engine,
            &mut linker,
            &mut store,
            component.as_ref(),
            "app",
            &explicit_dependency_names,
            capabilities,
            bindings,
            &available_plugins,
            None,
        )?;
        self.ensure_wasi_nn_component_support(component.as_ref())?;

        let instance = linker
            .instantiate_async(&mut store, component.as_ref())
            .await
            .map_err(|e| map_runtime_error(format!("rpc component instantiate failed: {e}")))?;
        let func = resolve_component_export_func(&mut store, &instance, interface_id, function)
            .map_err(|e| {
                map_runtime_error(format!(
                    "failed to resolve rpc export '{}.{}': {e}",
                    interface_id, function
                ))
            })?;

        if let Ok(typed_func) = func.typed::<(Vec<u8>,), (Vec<u8>,)>(&store) {
            let (result_bytes,) = typed_func
                .call_async(&mut store, (payload_cbor,))
                .await
                .map_err(|e| map_runtime_error(format!("rpc invoke trap: {e}")))?;
            return Ok(result_bytes);
        }

        if let Ok(typed_func) = func.typed::<(Vec<u8>,), (Result<Vec<u8>, String>,)>(&store) {
            let (result_value,) = typed_func
                .call_async(&mut store, (payload_cbor,))
                .await
                .map_err(|e| map_runtime_error(format!("rpc invoke trap: {e}")))?;
            return match result_value {
                Ok(result_bytes) => Ok(result_bytes),
                Err(message) => Err(map_runtime_error(format!(
                    "rpc invoke returned error: {message}"
                ))),
            };
        }

        let func_ty = func.ty(&store);
        let param_types = func_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
        let result_types = func_ty.results().collect::<Vec<_>>();
        let params = decode_payload_values(&payload_cbor, &param_types).map_err(|err| {
            map_runtime_error(format!(
                "failed to decode rpc payload for '{}.{}': {}",
                interface_id, function, err.message
            ))
        })?;
        let mut results = placeholder_values(&result_types)?;
        func.call_async(&mut store, &params, &mut results)
            .await
            .map_err(|e| map_runtime_error(format!("rpc invoke trap: {e}")))?;
        encode_payload_values(&results, &result_types).map_err(|err| {
            map_runtime_error(format!(
                "failed to encode rpc result payload: {}",
                err.message
            ))
        })
    }

    async fn invoke_rpc_component(
        &self,
        request: RuntimeInvokeRequest,
    ) -> Result<Vec<u8>, ImagodError> {
        let RuntimeInvokeRequest {
            context,
            interface_id,
            function,
            payload_cbor,
        } = request;

        if context.app_type != RunnerAppType::Rpc {
            return Err(map_runtime_error(format!(
                "rpc invoke is only allowed when app_type=rpc (got: {})",
                app_type_text(context.app_type)
            )));
        }

        let native_plugin_context = NativePluginContext::new(
            context.service_name.clone(),
            context.release_hash.clone(),
            context.runner_id.clone(),
            context.app_type,
            context.manager_control_endpoint.clone(),
            context.manager_auth_secret.clone(),
            context.resources.clone(),
        );
        self.invoke_rpc_component_async(
            &context.component_path,
            &context.args,
            &context.envs,
            &context.wasi_mounts,
            &context.wasi_http_outbound,
            native_plugin_context,
            &context.plugin_dependencies,
            &context.capabilities,
            &context.bindings,
            &interface_id,
            &function,
            payload_cbor,
        )
        .await
    }
}

fn resolve_component_export_func(
    mut store: impl wasmtime::AsContextMut<Data = WasiState>,
    instance: &wasmtime::component::Instance,
    interface_name: &str,
    function_name: &str,
) -> Result<Func, ImagodError> {
    let interface_index = instance
        .get_export_index(store.as_context_mut(), None, interface_name)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "rpc export interface '{}' was not found",
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
                "rpc export function '{}.{}' was not found",
                interface_name, function_name
            ))
        })?;
    instance
        .get_func(store.as_context_mut(), function_index)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "rpc export '{}.{}' is not a function",
                interface_name, function_name
            ))
        })
}

fn dispatch_http_work_item(
    request_tx: &mpsc::Sender<RuntimeHttpWorkItem>,
    request: RuntimeHttpRequest,
) -> Result<oneshot::Receiver<Result<RuntimeHttpResponse, ImagodError>>, ImagodError> {
    let (response_tx, response_rx) = oneshot::channel();
    match request_tx.try_send(RuntimeHttpWorkItem {
        request,
        response_tx,
    }) {
        Ok(()) => Ok(response_rx),
        Err(mpsc::error::TrySendError::Full(_)) => Err(ImagodError::new(
            ErrorCode::Busy,
            STAGE_RUNTIME,
            "http component worker queue is full",
        )),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(map_runtime_error(
            "http component worker request channel is closed".to_string(),
        )),
    }
}

fn http_request_queue_capacity(http_worker_count: u32, http_worker_queue_capacity: u32) -> usize {
    let _ = http_worker_count;
    let queue_capacity = usize::try_from(http_worker_queue_capacity)
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(HTTP_REQUEST_QUEUE_CAPACITY);
    queue_capacity.max(1)
}

#[async_trait]
impl ComponentRuntime for WasmRuntime {
    fn validate_component(&self, component_path: &Path) -> Result<(), ImagodError> {
        self.validate_component_loadable(component_path)
    }

    async fn run_component(&self, request: RuntimeRunRequest) -> Result<(), ImagodError> {
        let RuntimeRunRequest {
            app_type,
            runner_id,
            service_name,
            release_hash,
            component_path,
            args,
            envs,
            wasi_mounts,
            wasi_http_outbound,
            resources,
            socket,
            plugin_dependencies,
            capabilities,
            bindings,
            manager_control_endpoint,
            manager_auth_secret,
            shutdown,
            epoch_tick_interval_ms,
            http_worker_count,
            http_worker_queue_capacity,
            http_ready_tx,
        } = request;
        let native_plugin_context = NativePluginContext::new(
            service_name,
            release_hash,
            runner_id,
            app_type,
            manager_control_endpoint,
            manager_auth_secret,
            resources,
        );

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
                    &wasi_mounts,
                    &wasi_http_outbound,
                    None,
                    native_plugin_context.clone(),
                    &plugin_dependencies,
                    &capabilities,
                    &bindings,
                    shutdown,
                    epoch_tick_interval_ms,
                )
                .await
            }
            RunnerAppType::Rpc => {
                if socket.is_some() {
                    return Err(map_runtime_error(
                        "socket config is only allowed when app_type=socket".to_string(),
                    ));
                }
                self.run_rpc_component_async(shutdown).await
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
                    &wasi_mounts,
                    &wasi_http_outbound,
                    native_plugin_context.clone(),
                    &plugin_dependencies,
                    &capabilities,
                    &bindings,
                    shutdown,
                    epoch_tick_interval_ms,
                    http_worker_count,
                    http_worker_queue_capacity,
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
                    &wasi_mounts,
                    &wasi_http_outbound,
                    Some(socket),
                    native_plugin_context,
                    &plugin_dependencies,
                    &capabilities,
                    &bindings,
                    shutdown,
                    epoch_tick_interval_ms,
                )
                .await
            }
        }
    }

    async fn handle_http_request(
        &self,
        request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, ImagodError> {
        self.handle_http_request_async(request).await
    }
}

#[async_trait]
impl RuntimeInvoker for WasmRuntime {
    async fn invoke_component(
        &self,
        request: RuntimeInvokeRequest,
    ) -> Result<Vec<u8>, ImagodError> {
        self.invoke_rpc_component(request).await
    }
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

fn configure_wasi_mounts(
    builder: &mut WasiCtxBuilder,
    mounts: &[RunnerWasiMount],
) -> Result<(), ImagodError> {
    for mount in mounts {
        let (dir_perms, file_perms) = if mount.read_only {
            (DirPerms::READ, FilePerms::READ)
        } else {
            (DirPerms::all(), FilePerms::all())
        };
        builder
            .preopened_dir(
                &mount.host_path,
                mount.guest_path.as_str(),
                dir_perms,
                file_perms,
            )
            .map_err(|err| {
                map_runtime_error(format!(
                    "failed to preopen mount {} -> {}: {err}",
                    mount.host_path.display(),
                    mount.guest_path
                ))
            })?;
    }
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
    use imagod_ipc::{RunnerSocketConfig, RunnerSocketProtocol, RunnerWasiMount};
    use std::{
        collections::BTreeMap,
        fs,
        hint::black_box,
        net::SocketAddr,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };
    use tempfile::{Builder as TempDirBuilder, TempDir};
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module};
    use wit_parser::{ManglingAndAbi, Resolve};

    fn sample_socket_config() -> RunnerSocketConfig {
        RunnerSocketConfig {
            protocol: RunnerSocketProtocol::Udp,
            direction: RunnerSocketDirection::Inbound,
            listen_addr: "0.0.0.0".to_string(),
            listen_port: 514,
        }
    }

    fn sample_http_request() -> RuntimeHttpRequest {
        RuntimeHttpRequest {
            method: "GET".to_string(),
            uri: "/".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn allow_all_wasi_capabilities() -> CapabilityPolicy {
        CapabilityPolicy {
            privileged: false,
            deps: BTreeMap::new(),
            wasi: BTreeMap::from([("*".to_string(), vec!["*".to_string()])]),
        }
    }

    fn p95_micros(samples: &mut [u128]) -> u128 {
        assert!(!samples.is_empty(), "samples must not be empty");
        samples.sort_unstable();
        let index = (samples.len() - 1) * 95 / 100;
        samples[index]
    }

    fn legacy_prepare_payload(payload: &[u8]) -> Vec<u8> {
        payload.to_vec()
    }

    fn optimized_prepare_payload(payload: Vec<u8>) -> Vec<u8> {
        payload
    }

    #[test]
    fn wasm_engine_tuning_default_matches_runtime_defaults() {
        let tuning = WasmEngineTuning::default();
        assert_eq!(
            tuning.memory_reservation_bytes,
            DEFAULT_WASM_MEMORY_RESERVATION_BYTES
        );
        assert_eq!(
            tuning.memory_reservation_for_growth_bytes,
            DEFAULT_WASM_MEMORY_RESERVATION_FOR_GROWTH_BYTES
        );
        assert_eq!(
            tuning.memory_guard_size_bytes,
            DEFAULT_WASM_MEMORY_GUARD_SIZE_BYTES
        );
        assert_eq!(
            tuning.guard_before_linear_memory,
            DEFAULT_WASM_GUARD_BEFORE_LINEAR_MEMORY
        );
        assert_eq!(
            tuning.parallel_compilation,
            DEFAULT_WASM_PARALLEL_COMPILATION
        );
    }

    #[test]
    fn runtime_initializes_with_custom_wasm_engine_tuning() {
        let tuning = WasmEngineTuning {
            memory_reservation_bytes: 8 * 1024 * 1024,
            memory_reservation_for_growth_bytes: 4 * 1024 * 1024,
            memory_guard_size_bytes: 0,
            guard_before_linear_memory: false,
            parallel_compilation: true,
        };
        let runtime = WasmRuntime::new_with_tuning(tuning)
            .expect("runtime should initialize with custom tuning");
        runtime.increment_epoch();
    }

    #[test]
    fn validate_component_loadable_populates_component_cache_once() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let fixture = write_wasi_http_component("component-cache");

        runtime
            .validate_component_loadable(&fixture.component_path)
            .expect("component should be loadable");
        runtime
            .validate_component_loadable(&fixture.component_path)
            .expect("component should be loadable on second validation");

        let cache = runtime
            .component_cache
            .read()
            .expect("component cache lock should be available");
        assert_eq!(cache.len(), 1, "component cache should deduplicate by path");
        assert!(
            cache.contains_key(&fixture.component_path),
            "validated component path should be cached"
        );
    }

    #[test]
    fn validate_component_loadable_is_safe_under_concurrent_first_load() {
        let runtime = Arc::new(WasmRuntime::new().expect("runtime should initialize"));
        let fixture = write_wasi_http_component("component-cache-concurrent");

        std::thread::scope(|scope| {
            for _ in 0..8 {
                let runtime = runtime.clone();
                let component_path = fixture.component_path.clone();
                scope.spawn(move || {
                    runtime
                        .validate_component_loadable(&component_path)
                        .expect("component should be loadable");
                });
            }
        });

        let cache = runtime
            .component_cache
            .read()
            .expect("component cache lock should be available");
        assert_eq!(cache.len(), 1, "component cache should deduplicate by path");
        assert!(
            cache.contains_key(&fixture.component_path),
            "validated component path should be cached"
        );
    }

    #[test]
    fn http_request_queue_capacity_uses_configured_queue_capacity_only() {
        assert_eq!(http_request_queue_capacity(1, 4), 4);
        assert_eq!(http_request_queue_capacity(4, 4), 4);
        assert_eq!(
            http_request_queue_capacity(0, 0),
            HTTP_REQUEST_QUEUE_CAPACITY
        );
    }

    #[test]
    #[ignore]
    fn rpc_payload_move_perf_compare() {
        const PAYLOAD_BYTES: usize = 1024 * 1024;
        const ITERATIONS: usize = 64;
        let payloads = (0..ITERATIONS)
            .map(|_| vec![0xCD; PAYLOAD_BYTES])
            .collect::<Vec<_>>();

        let mut legacy_samples = Vec::with_capacity(ITERATIONS);
        for payload in &payloads {
            let started = Instant::now();
            let prepared = legacy_prepare_payload(payload);
            black_box(prepared);
            legacy_samples.push(started.elapsed().as_micros());
        }

        let mut optimized_samples = Vec::with_capacity(ITERATIONS);
        for payload in payloads {
            let started = Instant::now();
            let prepared = optimized_prepare_payload(payload);
            black_box(prepared);
            optimized_samples.push(started.elapsed().as_micros());
        }

        let legacy_p95 = p95_micros(&mut legacy_samples);
        let optimized_p95 = p95_micros(&mut optimized_samples);
        eprintln!(
            "rpc_payload_move_perf_compare payload_bytes={} iterations={} optimized_p95_us={} legacy_p95_us={}",
            PAYLOAD_BYTES, ITERATIONS, optimized_p95, legacy_p95
        );
    }

    struct WasiHttpFixture {
        _temp_dir: TempDir,
        component_path: PathBuf,
    }

    fn write_wasi_http_component(prefix: &str) -> WasiHttpFixture {
        write_wasi_http_component_with_version(prefix, "0.2.4")
    }

    fn write_wasi_http_component_with_version(prefix: &str, version: &str) -> WasiHttpFixture {
        let temp_dir = TempDirBuilder::new()
            .prefix(&format!("imago-runtime-wasi-http-{prefix}-"))
            .tempdir()
            .expect("temp dir should be created");
        let root = temp_dir.path();
        let http_deps_dir = root.join("deps/http");
        fs::create_dir_all(&http_deps_dir).expect("http deps directory should be created");
        fs::write(
            root.join("world.wit"),
            format!(
                r#"
package test:wasi-http-import@0.1.0;

world app {{
  import wasi:http/types@{version};
}}
"#,
            ),
        )
        .expect("wit source should be written");
        fs::write(
            http_deps_dir.join("package.wit"),
            format!(
                r#"
package wasi:http@{version};

interface types {{
  type field-key = string;
  type field-name = field-key;

  resource fields {{
    constructor();
    has: func(name: field-name) -> bool;
  }}
}}
"#,
            ),
        )
        .expect("wasi:http fixture should be written");

        let mut resolve = Resolve::default();
        let (pkg, _) = resolve
            .push_dir(root)
            .expect("fixture WIT directory should parse");
        let world = resolve
            .select_world(&[pkg], Some("app"))
            .expect("world 'app' should exist");
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .expect("component metadata embedding should succeed");
        let component = ComponentEncoder::default()
            .module(&module)
            .expect("component encoder should accept module")
            .encode()
            .expect("component encoding should succeed");
        let component_path = root.join("component.wasm");
        fs::write(&component_path, component).expect("component bytes should be written");
        WasiHttpFixture {
            _temp_dir: temp_dir,
            component_path,
        }
    }

    fn write_wasi_http_incoming_handler_component(prefix: &str) -> WasiHttpFixture {
        let temp_dir = TempDirBuilder::new()
            .prefix(&format!("imago-runtime-wasi-http-handler-{prefix}-"))
            .tempdir()
            .expect("temp dir should be created");
        let root = temp_dir.path();
        let deps_dir = root.join("deps");
        fs::create_dir_all(&deps_dir).expect("http deps directory should be created");
        fs::write(
            root.join("world.wit"),
            r#"
package test:wasi-http-handler@0.1.0;

world app {
  export wasi:http/incoming-handler@0.2.6;
}
"#,
        )
        .expect("wit source should be written");
        let repo_deps_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/imago-with-componentize-js-hono/wit/deps");
        for dep_dir in [
            "wasi-cli-0.2.6",
            "wasi-clocks-0.2.6",
            "wasi-http-0.2.6",
            "wasi-io-0.2.6",
            "wasi-random-0.2.6",
        ] {
            let source = repo_deps_dir.join(dep_dir).join("package.wit");
            let dest_dir = deps_dir.join(dep_dir);
            fs::create_dir_all(&dest_dir).expect("dependency directory should be created");
            fs::copy(&source, dest_dir.join("package.wit")).unwrap_or_else(|err| {
                panic!("failed to copy {}: {err}", source.display());
            });
        }

        let mut resolve = Resolve::default();
        let (pkg, _) = resolve
            .push_dir(root)
            .expect("fixture WIT directory should parse");
        let world = resolve
            .select_world(&[pkg], Some("app"))
            .expect("world 'app' should exist");
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .expect("component metadata embedding should succeed");
        let component = ComponentEncoder::default()
            .module(&module)
            .expect("component encoder should accept module")
            .encode()
            .expect("component encoding should succeed");
        let component_path = root.join("component.wasm");
        fs::write(&component_path, component).expect("component bytes should be written");
        WasiHttpFixture {
            _temp_dir: temp_dir,
            component_path,
        }
    }

    struct WasiNnFixture {
        _temp_dir: TempDir,
        component_path: PathBuf,
    }

    fn write_wasi_nn_component(prefix: &str) -> WasiNnFixture {
        let temp_dir = TempDirBuilder::new()
            .prefix(&format!("imago-runtime-wasi-nn-{prefix}-"))
            .tempdir()
            .expect("temp dir should be created");
        let root = temp_dir.path();
        let nn_deps_dir = root.join("deps/nn");
        fs::create_dir_all(&nn_deps_dir).expect("wasi:nn deps directory should be created");
        fs::write(
            root.join("world.wit"),
            r#"
package test:wasi-nn-import@0.1.0;

world app {
  import wasi:nn/graph@0.2.0-rc-2024-10-28;
}
"#,
        )
        .expect("wit source should be written");
        fs::write(
            nn_deps_dir.join("package.wit"),
            r#"
package wasi:nn@0.2.0-rc-2024-10-28;

interface errors {
  enum error-code {
    invalid-argument,
    invalid-encoding,
    timeout,
    runtime-error,
    unsupported-operation,
    too-large,
    not-found,
    security,
    unknown,
  }

  resource error {
    code: func() -> error-code;
    data: func() -> string;
  }
}

interface tensor {
  type tensor-dimensions = list<u32>;
  enum tensor-type {
    FP16,
    FP32,
    FP64,
    BF16,
    U8,
    I32,
    I64,
  }
  type tensor-data = list<u8>;

  resource tensor {
    constructor(dimensions: tensor-dimensions, ty: tensor-type, data: tensor-data);
  }
}

interface inference {
  use errors.{error};
  use tensor.{tensor};

  type named-tensor = tuple<string, tensor>;

  resource graph-execution-context {
    compute: func(inputs: list<named-tensor>) -> result<list<named-tensor>, error>;
  }
}

interface graph {
  use errors.{error};
  use inference.{graph-execution-context};

  resource graph {
    init-execution-context: func() -> result<graph-execution-context, error>;
  }

  enum graph-encoding {
    openvino,
    onnx,
    tensorflow,
    pytorch,
    tensorflowlite,
    ggml,
    autodetect,
  }

  enum execution-target {
    cpu,
    gpu,
    tpu,
  }

  type graph-builder = list<u8>;

  load: func(builder: list<graph-builder>, encoding: graph-encoding, target: execution-target) -> result<graph, error>;
  load-by-name: func(name: string) -> result<graph, error>;
}
"#,
        )
        .expect("wasi:nn fixture should be written");

        let mut resolve = Resolve::default();
        let (pkg, _) = resolve
            .push_dir(root)
            .expect("fixture WIT directory should parse");
        let world = resolve
            .select_world(&[pkg], Some("app"))
            .expect("world 'app' should exist");
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        wit_component::embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
            .expect("component metadata embedding should succeed");
        let component = ComponentEncoder::default()
            .module(&module)
            .expect("component encoder should accept module")
            .encode()
            .expect("component encoding should succeed");
        let component_path = root.join("component.wasm");
        fs::write(&component_path, component).expect("component bytes should be written");
        WasiNnFixture {
            _temp_dir: temp_dir,
            component_path,
        }
    }

    fn collect_component_import_names(runtime: &WasmRuntime, component_path: &Path) -> Vec<String> {
        let component =
            Component::from_file(&runtime.engine, component_path).expect("component should load");
        component
            .component_type()
            .imports(&runtime.engine)
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>()
    }

    #[tokio::test]
    async fn queue_full_dispatch_returns_busy_without_awaiting() {
        let (request_tx, mut request_rx) = mpsc::channel::<RuntimeHttpWorkItem>(1);
        request_tx
            .try_send(RuntimeHttpWorkItem {
                request: sample_http_request(),
                response_tx: oneshot::channel().0,
            })
            .expect("first enqueue should fill queue");

        let err = dispatch_http_work_item(&request_tx, sample_http_request())
            .expect_err("second enqueue should fail with busy");
        assert_eq!(err.code, ErrorCode::Busy);
        assert!(err.message.contains("queue is full"));

        let _ = request_rx.recv().await;
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
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
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
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: Some(sample_socket_config()),
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
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
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
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
    async fn http_type_accepts_wasi_http_incoming_handler_0_2_6_component() {
        let fixture = write_wasi_http_incoming_handler_component("http-0-2-6");
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let imports = collect_component_import_names(&runtime, &fixture.component_path);
        assert!(
            imports.iter().any(|name| name == "wasi:http/types@0.2.6"),
            "component should import wasi:http/types@0.2.6, got: {imports:?}"
        );

        let component_path = fixture.component_path.clone();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = oneshot::channel();
        let run_task = tokio::spawn(async move {
            runtime
                .run_component(RuntimeRunRequest {
                    app_type: RunnerAppType::Http,
                    runner_id: "runner-test".to_string(),
                    service_name: "svc-test".to_string(),
                    release_hash: "release-test".to_string(),
                    component_path,
                    args: Vec::new(),
                    envs: BTreeMap::new(),
                    wasi_mounts: Vec::new(),
                    wasi_http_outbound: Vec::new(),
                    resources: std::collections::BTreeMap::new(),
                    socket: None,
                    plugin_dependencies: Vec::new(),
                    capabilities: allow_all_wasi_capabilities(),
                    bindings: Vec::new(),
                    manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                    manager_auth_secret: "secret".to_string(),
                    shutdown: shutdown_rx,
                    epoch_tick_interval_ms: 50,
                    http_worker_count: 2,
                    http_worker_queue_capacity: 4,
                    http_ready_tx: Some(ready_tx),
                })
                .await
        });

        match tokio::time::timeout(Duration::from_secs(2), ready_rx)
            .await
            .expect("http component should register in time")
        {
            Ok(()) => {}
            Err(err) => {
                let task_result = tokio::time::timeout(Duration::from_secs(2), run_task)
                    .await
                    .expect("http component task should complete after ready channel failure")
                    .expect("http component task should not panic");
                panic!(
                    "http ready channel should succeed: {err}; runtime returned: {:?}",
                    task_result
                );
            }
        }
        shutdown_tx
            .send(true)
            .expect("shutdown sender should still be connected");
        tokio::time::timeout(Duration::from_secs(2), run_task)
            .await
            .expect("http component should stop in time")
            .expect("http component task should complete")
            .expect("http component should stop without error");
    }

    #[tokio::test]
    async fn cli_type_with_wasi_http_import_does_not_fail_with_missing_http_linker() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let fixture = write_wasi_http_component("cli-http-linker");
        let imports = collect_component_import_names(&runtime, &fixture.component_path);
        assert!(
            imports.iter().any(|name| name == "wasi:http/types@0.2.4"),
            "component should import wasi:http/types@0.2.4, got: {imports:?}"
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Cli,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: fixture.component_path.clone(),
                args: Vec::new(),
                envs: BTreeMap::new(),
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: allow_all_wasi_capabilities(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
                http_ready_tx: None,
            })
            .await
            .expect_err("component without wasi:cli/run export should fail");
        let _ = shutdown_tx.send(true);
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            !err.message
                .contains("matching implementation was not found in the linker"),
            "unexpected missing-linker error: {}",
            err.message
        );
    }

    #[cfg(any(
        feature = "wasi-nn-cvitek",
        feature = "wasi-nn-openvino",
        feature = "wasi-nn-onnx"
    ))]
    #[tokio::test]
    async fn cli_type_with_wasi_nn_import_does_not_fail_with_missing_nn_linker() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let fixture = write_wasi_nn_component("cli-nn-linker");
        let imports = collect_component_import_names(&runtime, &fixture.component_path);
        assert!(
            imports
                .iter()
                .any(|name| name == "wasi:nn/graph@0.2.0-rc-2024-10-28"),
            "component should import wasi:nn/graph@0.2.0-rc-2024-10-28, got: {imports:?}"
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Cli,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: fixture.component_path.clone(),
                args: Vec::new(),
                envs: BTreeMap::new(),
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: allow_all_wasi_capabilities(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
                http_ready_tx: None,
            })
            .await
            .expect_err("component without wasi:cli/run export should fail");
        let _ = shutdown_tx.send(true);
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            !err.message
                .contains("matching implementation was not found in the linker"),
            "unexpected missing-linker error: {}",
            err.message
        );
    }

    #[cfg(not(any(
        feature = "wasi-nn-cvitek",
        feature = "wasi-nn-openvino",
        feature = "wasi-nn-onnx"
    )))]
    #[tokio::test]
    async fn cli_type_with_wasi_nn_import_requires_backend_feature() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let fixture = write_wasi_nn_component("cli-nn-disabled");
        let imports = collect_component_import_names(&runtime, &fixture.component_path);
        assert!(
            imports
                .iter()
                .any(|name| name == "wasi:nn/graph@0.2.0-rc-2024-10-28"),
            "component should import wasi:nn/graph@0.2.0-rc-2024-10-28, got: {imports:?}"
        );

        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Cli,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: fixture.component_path.clone(),
                args: Vec::new(),
                envs: BTreeMap::new(),
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: allow_all_wasi_capabilities(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
                http_ready_tx: None,
            })
            .await
            .expect_err("missing backend feature should reject wasi:nn import");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("wasi-nn backend is not enabled"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn rpc_type_returns_without_loading_component_when_shutdown_already_signaled() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);
        runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Rpc,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: PathBuf::from("/tmp/non-existent-rpc-component.wasm"),
                args: Vec::new(),
                envs: BTreeMap::new(),
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
                http_ready_tx: None,
            })
            .await
            .expect("rpc run should exit cleanly when shutdown is already signaled");
    }

    #[tokio::test]
    async fn rpc_type_waits_for_shutdown_signal() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let run_future = runtime.run_component(RuntimeRunRequest {
            app_type: RunnerAppType::Rpc,
            runner_id: "runner-test".to_string(),
            service_name: "svc-test".to_string(),
            release_hash: "release-test".to_string(),
            component_path: PathBuf::from("/tmp/non-existent-rpc-component.wasm"),
            args: Vec::new(),
            envs: BTreeMap::new(),
            wasi_mounts: Vec::new(),
            wasi_http_outbound: Vec::new(),
            resources: std::collections::BTreeMap::new(),
            socket: None,
            plugin_dependencies: Vec::new(),
            capabilities: CapabilityPolicy::default(),
            bindings: Vec::new(),
            manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
            manager_auth_secret: "secret".to_string(),
            shutdown: shutdown_rx,
            epoch_tick_interval_ms: 50,
            http_worker_count: 2,
            http_worker_queue_capacity: 4,
            http_ready_tx: None,
        });
        tokio::pin!(run_future);

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut run_future)
                .await
                .is_err(),
            "rpc runner should stay alive until shutdown is signaled"
        );

        shutdown_tx
            .send(true)
            .expect("shutdown sender should still be connected");
        tokio::time::timeout(Duration::from_secs(1), &mut run_future)
            .await
            .expect("rpc runner should stop promptly after shutdown")
            .expect("rpc run should stop without error");
    }

    #[tokio::test]
    async fn rpc_invoke_with_wasi_http_import_does_not_fail_with_missing_http_linker() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let fixture = write_wasi_http_component("rpc-http-linker");
        let imports = collect_component_import_names(&runtime, &fixture.component_path);
        assert!(
            imports.iter().any(|name| name == "wasi:http/types@0.2.4"),
            "component should import wasi:http/types@0.2.4, got: {imports:?}"
        );

        let err = runtime
            .invoke_component(RuntimeInvokeRequest {
                context: Arc::new(RuntimeInvokeContext {
                    app_type: RunnerAppType::Rpc,
                    runner_id: "runner-test".to_string(),
                    service_name: "svc-test".to_string(),
                    release_hash: "release-test".to_string(),
                    component_path: fixture.component_path.clone(),
                    args: Vec::new(),
                    envs: BTreeMap::new(),
                    wasi_mounts: Vec::new(),
                    wasi_http_outbound: Vec::new(),
                    resources: std::collections::BTreeMap::new(),
                    plugin_dependencies: Vec::new(),
                    capabilities: allow_all_wasi_capabilities(),
                    bindings: Vec::new(),
                    manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                    manager_auth_secret: "secret".to_string(),
                }),
                interface_id: "missing:iface/run@0.1.0".to_string(),
                function: "invoke".to_string(),
                payload_cbor: Vec::new(),
            })
            .await
            .expect_err("component without rpc export should fail after instantiate");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("rpc export interface"),
            "unexpected message: {}",
            err.message
        );
        assert!(
            !err.message
                .contains("matching implementation was not found in the linker"),
            "unexpected missing-linker error: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn cli_type_with_wasi_http_import_stays_unauthorized_without_capability() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let fixture = write_wasi_http_component("cli-http-unauthorized");
        let imports = collect_component_import_names(&runtime, &fixture.component_path);
        assert!(
            imports.iter().any(|name| name == "wasi:http/types@0.2.4"),
            "component should import wasi:http/types@0.2.4, got: {imports:?}"
        );

        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Cli,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: fixture.component_path.clone(),
                args: Vec::new(),
                envs: BTreeMap::new(),
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
                http_ready_tx: None,
            })
            .await
            .expect_err("missing capability should reject wasi:http import");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert!(
            err.message.contains("capability denied"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn cli_type_with_wasi_nn_import_stays_unauthorized_without_capability() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let fixture = write_wasi_nn_component("cli-nn-unauthorized");
        let imports = collect_component_import_names(&runtime, &fixture.component_path);
        assert!(
            imports
                .iter()
                .any(|name| name == "wasi:nn/graph@0.2.0-rc-2024-10-28"),
            "component should import wasi:nn/graph@0.2.0-rc-2024-10-28, got: {imports:?}"
        );

        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Cli,
                runner_id: "runner-test".to_string(),
                service_name: "svc-test".to_string(),
                release_hash: "release-test".to_string(),
                component_path: fixture.component_path.clone(),
                args: Vec::new(),
                envs: BTreeMap::new(),
                wasi_mounts: Vec::new(),
                wasi_http_outbound: Vec::new(),
                resources: std::collections::BTreeMap::new(),
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
                bindings: Vec::new(),
                manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
                manager_auth_secret: "secret".to_string(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
                http_worker_count: 2,
                http_worker_queue_capacity: 4,
                http_ready_tx: None,
            })
            .await
            .expect_err("missing capability should reject wasi:nn import");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert!(
            err.message.contains("capability denied"),
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

    #[test]
    fn configure_wasi_mounts_accepts_read_write_and_read_only() {
        let temp_dir = TempDirBuilder::new()
            .prefix("imago-runtime-wasi-mount-permissions-")
            .tempdir()
            .expect("temp dir should be created");
        let rw = temp_dir.path().join("rw");
        let ro = temp_dir.path().join("ro");
        fs::create_dir_all(&rw).expect("rw dir should be created");
        fs::create_dir_all(&ro).expect("ro dir should be created");

        let mut builder = WasiCtxBuilder::new();
        configure_wasi_mounts(
            &mut builder,
            &[
                RunnerWasiMount {
                    host_path: rw,
                    guest_path: "/guest/rw".to_string(),
                    read_only: false,
                },
                RunnerWasiMount {
                    host_path: ro,
                    guest_path: "/guest/ro".to_string(),
                    read_only: true,
                },
            ],
        )
        .expect("mount configuration should succeed");
    }
}
