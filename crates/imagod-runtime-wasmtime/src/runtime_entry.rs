use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{
    CapabilityPolicy, PluginDependency, RunnerAppType, RunnerSocketConfig, RunnerSocketDirection,
};
use imagod_runtime_internal::{
    ComponentRuntime, HttpComponentSupervisor, PluginResolver, RuntimeHttpRequest,
    RuntimeHttpResponse, RuntimeHttpWorkItem, RuntimeRunRequest,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Linker},
};
use wasmtime_wasi::{
    WasiCtxBuilder,
    p2::{add_to_linker_async, bindings::Command},
    sockets::SocketAddrUse,
};
use wasmtime_wasi_http::{add_only_http_to_linker_async, bindings::Proxy};

use crate::{
    HTTP_REQUEST_QUEUE_CAPACITY, NativePluginContext, STAGE_RUNTIME, WasiState,
    capability_checker::DefaultCapabilityChecker,
    http_supervisor::{DefaultHttpComponentSupervisor, run_http_worker},
    map_runtime_error,
    native_plugins::NativePluginRegistry,
    plugin_resolver::{
        DefaultPluginResolver, instantiate_plugin_dependencies, register_plugin_import_shims,
    },
};

/// Runner-local wrapper around a configured Wasmtime engine.
pub struct WasmRuntime {
    engine: Arc<Engine>,
    native_plugins: NativePluginRegistry,
    plugin_resolver: Arc<DefaultPluginResolver>,
    http_supervisor: Arc<DefaultHttpComponentSupervisor>,
    capability_checker: Arc<DefaultCapabilityChecker>,
}

impl Clone for WasmRuntime {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
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
        Self::new_with_native_plugins(NativePluginRegistry::default())
    }

    /// Creates a runtime with a native plugin registry injected by manager build.
    pub fn new_with_native_plugins(
        native_plugins: NativePluginRegistry,
    ) -> Result<Self, ImagodError> {
        Self::new_with_runtime_contracts(
            native_plugins,
            Arc::new(DefaultPluginResolver),
            Arc::new(DefaultHttpComponentSupervisor::new()),
            Arc::new(DefaultCapabilityChecker),
        )
    }

    fn new_with_runtime_contracts(
        native_plugins: NativePluginRegistry,
        plugin_resolver: Arc<DefaultPluginResolver>,
        http_supervisor: Arc<DefaultHttpComponentSupervisor>,
        capability_checker: Arc<DefaultCapabilityChecker>,
    ) -> Result<Self, ImagodError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        config.epoch_interruption(true);

        let engine = Engine::new(&config)
            .map_err(|e| map_runtime_error(format!("engine init failed: {e}")))?;

        Ok(Self {
            engine: Arc::new(engine),
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
            table: wasmtime::component::ResourceTable::new(),
            wasi: builder.build(),
            http: wasmtime_wasi_http::WasiHttpCtx::new(),
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

    #[allow(clippy::too_many_arguments)]
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

    #[allow(clippy::too_many_arguments)]
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
        http_worker_count: u32,
        http_worker_queue_capacity: u32,
        http_ready_tx: Option<oneshot::Sender<()>>,
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
    let worker_count = usize::try_from(http_worker_count)
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(1);
    let queue_capacity = usize::try_from(http_worker_queue_capacity)
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(HTTP_REQUEST_QUEUE_CAPACITY);
    worker_count.saturating_mul(queue_capacity).max(1)
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
            socket,
            plugin_dependencies,
            capabilities,
            shutdown,
            epoch_tick_interval_ms,
            http_worker_count,
            http_worker_queue_capacity,
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
    }

    async fn handle_http_request(
        &self,
        request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, ImagodError> {
        self.handle_http_request_async(request).await
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
    use imagod_ipc::{RunnerSocketConfig, RunnerSocketProtocol};
    use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

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
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
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
                socket: Some(sample_socket_config()),
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
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
                socket: None,
                plugin_dependencies: Vec::new(),
                capabilities: CapabilityPolicy::default(),
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
