//! Runner process bootstrap and orchestration.

use std::sync::Arc;

#[cfg(not(feature = "runtime-wasmtime"))]
use async_trait::async_trait;
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::RunnerAppType;
use imagod_runtime_bootstrap::{
    STAGE_RUNNER, SocketCleanupGuard, prepare_socket_path, read_runner_bootstrap,
};
use imagod_runtime_control::{
    DbusRunnerManagerClient, mark_ready, register, run_inbound_server, send_heartbeats,
};
use imagod_runtime_ingress::{STAGE_HTTP_INGRESS, spawn_http_ingress_server};
use tokio::{
    net::UnixListener,
    sync::{oneshot, watch},
    task::JoinHandle,
    time::{self, Duration},
};

use crate::NativePluginRegistry;
#[cfg(feature = "runtime-wasmtime")]
use crate::WasmRuntime;
use crate::runtime::{ComponentRuntime, RuntimeInvoker, RuntimeRunRequest};
#[cfg(not(feature = "runtime-wasmtime"))]
use crate::runtime::{RuntimeHttpRequest, RuntimeHttpResponse, RuntimeInvokeRequest};

const STARTUP_CONFIRM_WINDOW: Duration = Duration::from_millis(200);

#[cfg(feature = "runtime-wasmtime")]
type RuntimeBackend = WasmRuntime;

#[cfg(not(feature = "runtime-wasmtime"))]
#[derive(Clone, Default)]
struct RuntimeBackend;

#[cfg(not(feature = "runtime-wasmtime"))]
#[async_trait]
impl ComponentRuntime for RuntimeBackend {
    fn validate_component(&self, _component_path: &std::path::Path) -> Result<(), ImagodError> {
        Err(runtime_backend_unavailable_error())
    }

    async fn run_component(&self, _request: RuntimeRunRequest) -> Result<(), ImagodError> {
        Err(runtime_backend_unavailable_error())
    }

    async fn handle_http_request(
        &self,
        _request: RuntimeHttpRequest,
    ) -> Result<RuntimeHttpResponse, ImagodError> {
        Err(runtime_backend_unavailable_error())
    }
}

#[cfg(not(feature = "runtime-wasmtime"))]
fn runtime_backend_unavailable_error() -> ImagodError {
    ImagodError::new(
        ErrorCode::Internal,
        STAGE_RUNNER,
        "runtime backend is not enabled; enable feature 'runtime-wasmtime'",
    )
}

#[cfg(not(feature = "runtime-wasmtime"))]
#[async_trait]
impl RuntimeInvoker for RuntimeBackend {
    async fn invoke_component(
        &self,
        _request: RuntimeInvokeRequest,
    ) -> Result<Vec<u8>, ImagodError> {
        Err(runtime_backend_unavailable_error())
    }
}

/// Startup observation result for runner workload task.
enum StartupRunState {
    /// Workload is still running after startup confirmation window.
    StillRunning,
    /// Workload exited during startup confirmation window.
    Finished(Result<(), ImagodError>),
}

/// HTTP runtime initialization observation result.
enum HttpRuntimeReadyState {
    /// Runtime initialized HTTP component and is ready to accept requests.
    Ready,
    /// Runtime task exited before becoming ready.
    Finished(Result<(), ImagodError>),
}

/// Starts runner mode by reading `RunnerBootstrap` from stdin and executing the component.
///
/// The function registers the runner with manager, serves inbound IPC requests,
/// emits heartbeat signals, and exits when the component finishes or shutdown is requested.
pub async fn run_runner_from_stdin() -> Result<(), ImagodError> {
    run_runner_from_stdin_with_registry(NativePluginRegistry::default()).await
}

/// Starts runner mode with a caller-provided native plugin registry.
pub async fn run_runner_from_stdin_with_registry(
    native_plugin_registry: NativePluginRegistry,
) -> Result<(), ImagodError> {
    let bootstrap = read_runner_bootstrap(tokio::io::stdin()).await?;

    prepare_socket_path(&bootstrap.runner_endpoint)?;
    let listener = UnixListener::bind(&bootstrap.runner_endpoint).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_RUNNER,
            format!(
                "failed to bind runner endpoint {}: {e}",
                bootstrap.runner_endpoint.display()
            ),
        )
    })?;
    let _socket_cleanup_guard = SocketCleanupGuard::new(bootstrap.runner_endpoint.clone());

    let runtime = create_runtime_backend(&native_plugin_registry)?;
    runtime.validate_component(&bootstrap.component_path)?;

    let manager_client = DbusRunnerManagerClient;
    register(&bootstrap, &manager_client).await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let runtime_invoker: Arc<dyn RuntimeInvoker> = runtime.clone();
    let inbound_task = tokio::spawn(run_inbound_server(
        listener,
        bootstrap.clone(),
        runtime_invoker.clone(),
        shutdown_tx.clone(),
        shutdown_rx.clone(),
    ));

    let (http_ready_tx, http_ready_rx) = if bootstrap.app_type == RunnerAppType::Http {
        let (tx, rx) = oneshot::channel::<()>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let runtime_for_run = runtime.clone();
    let bootstrap_for_run = bootstrap.clone();
    let mut run_task = tokio::spawn(async move {
        runtime_for_run
            .run_component(RuntimeRunRequest {
                app_type: bootstrap_for_run.app_type,
                runner_id: bootstrap_for_run.runner_id.clone(),
                service_name: bootstrap_for_run.service_name.clone(),
                release_hash: bootstrap_for_run.release_hash.clone(),
                component_path: bootstrap_for_run.component_path.clone(),
                args: bootstrap_for_run.args.clone(),
                envs: bootstrap_for_run.envs.clone(),
                wasi_mounts: bootstrap_for_run.wasi_mounts.clone(),
                wasi_http_outbound: bootstrap_for_run.wasi_http_outbound.clone(),
                socket: bootstrap_for_run.socket.clone(),
                plugin_dependencies: bootstrap_for_run.plugin_dependencies.clone(),
                capabilities: bootstrap_for_run.capabilities.clone(),
                bindings: bootstrap_for_run.bindings.clone(),
                manager_control_endpoint: bootstrap_for_run.manager_control_endpoint.clone(),
                manager_auth_secret: bootstrap_for_run.manager_auth_secret.clone(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: bootstrap_for_run.epoch_tick_interval_ms,
                http_worker_count: bootstrap_for_run.http_worker_count,
                http_worker_queue_capacity: bootstrap_for_run.http_worker_queue_capacity,
                http_ready_tx,
            })
            .await
    });

    let http_ingress_task = match bootstrap.app_type {
        RunnerAppType::Http => {
            let ready_rx = http_ready_rx.expect("http ready receiver should exist");
            match wait_http_runtime_ready_or_exit(&mut run_task, ready_rx).await? {
                HttpRuntimeReadyState::Finished(run_result) => {
                    if run_result.is_ok()
                        && let Err(err) = mark_ready(&bootstrap, &manager_client).await
                    {
                        let _ = shutdown_runner_tasks(&shutdown_tx, inbound_task, None, None, None)
                            .await;
                        return Err(err);
                    }
                    let _ =
                        shutdown_runner_tasks(&shutdown_tx, inbound_task, None, None, None).await;
                    return run_result;
                }
                HttpRuntimeReadyState::Ready => {}
            }

            let ingress = match spawn_http_ingress_server(
                runtime.clone(),
                bootstrap.clone(),
                shutdown_tx.clone(),
                shutdown_tx.subscribe(),
            )
            .await
            {
                Ok(v) => v,
                Err(err) => {
                    let _ = shutdown_runner_tasks(
                        &shutdown_tx,
                        inbound_task,
                        None,
                        None,
                        Some(run_task),
                    )
                    .await;
                    return Err(err);
                }
            };

            if let Err(err) = mark_ready(&bootstrap, &manager_client).await {
                let _ = shutdown_runner_tasks(
                    &shutdown_tx,
                    inbound_task,
                    Some(ingress),
                    None,
                    Some(run_task),
                )
                .await;
                return Err(err);
            }

            Some(ingress)
        }
        RunnerAppType::Cli | RunnerAppType::Rpc | RunnerAppType::Socket => None,
    };

    if bootstrap.app_type != RunnerAppType::Http {
        match observe_startup_window(&mut run_task, STARTUP_CONFIRM_WINDOW).await? {
            StartupRunState::Finished(run_result) => {
                if run_result.is_ok()
                    && let Err(err) = mark_ready(&bootstrap, &manager_client).await
                {
                    let _ = shutdown_runner_tasks(
                        &shutdown_tx,
                        inbound_task,
                        http_ingress_task,
                        None,
                        None,
                    )
                    .await;
                    return Err(err);
                }
                let http_result = shutdown_runner_tasks(
                    &shutdown_tx,
                    inbound_task,
                    http_ingress_task,
                    None,
                    None,
                )
                .await;
                if let Some(Err(err)) = http_result {
                    return Err(err);
                }
                return run_result;
            }
            StartupRunState::StillRunning => {}
        }

        if let Err(err) = mark_ready(&bootstrap, &manager_client).await {
            let _ = shutdown_runner_tasks(
                &shutdown_tx,
                inbound_task,
                http_ingress_task,
                None,
                Some(run_task),
            )
            .await;
            return Err(err);
        }
    }

    let heartbeat_task = tokio::spawn(send_heartbeats(
        bootstrap.clone(),
        manager_client.clone(),
        shutdown_tx.clone(),
        shutdown_tx.subscribe(),
    ));
    let run_result = join_run_task(run_task).await;
    let http_result = shutdown_runner_tasks(
        &shutdown_tx,
        inbound_task,
        http_ingress_task,
        Some(heartbeat_task),
        None,
    )
    .await;
    if let Some(Err(err)) = http_result {
        return Err(err);
    }

    run_result
}

fn create_runtime_backend(
    native_plugin_registry: &NativePluginRegistry,
) -> Result<Arc<RuntimeBackend>, ImagodError> {
    #[cfg(feature = "runtime-wasmtime")]
    {
        Ok(Arc::new(WasmRuntime::new_with_native_plugins(
            native_plugin_registry.clone(),
        )?))
    }

    #[cfg(not(feature = "runtime-wasmtime"))]
    {
        let _ = native_plugin_registry;
        Err(runtime_backend_unavailable_error())
    }
}

/// Observes run task during startup confirmation window.
async fn observe_startup_window(
    run_task: &mut JoinHandle<Result<(), ImagodError>>,
    window: Duration,
) -> Result<StartupRunState, ImagodError> {
    tokio::select! {
        joined = run_task => {
            let run_result = joined.map_err(map_run_join_error)?;
            Ok(StartupRunState::Finished(run_result))
        }
        _ = time::sleep(window) => Ok(StartupRunState::StillRunning),
    }
}

/// Waits until HTTP runtime reports readiness or exits early.
async fn wait_http_runtime_ready_or_exit(
    run_task: &mut JoinHandle<Result<(), ImagodError>>,
    http_ready_rx: oneshot::Receiver<()>,
) -> Result<HttpRuntimeReadyState, ImagodError> {
    tokio::select! {
        joined = run_task => {
            let run_result = joined.map_err(map_run_join_error)?;
            Ok(HttpRuntimeReadyState::Finished(run_result))
        }
        ready = http_ready_rx => {
            ready.map_err(|_| {
                ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_RUNNER,
                    "http runtime initialization ended before ready signal",
                )
            })?;
            Ok(HttpRuntimeReadyState::Ready)
        }
    }
}

/// Joins workload run task and maps join errors to internal runner errors.
async fn join_run_task(run_task: JoinHandle<Result<(), ImagodError>>) -> Result<(), ImagodError> {
    run_task.await.map_err(map_run_join_error)?
}

/// Signals shutdown and waits for remaining background tasks.
async fn shutdown_runner_tasks(
    shutdown_tx: &watch::Sender<bool>,
    inbound_task: JoinHandle<()>,
    http_ingress_task: Option<JoinHandle<Result<(), ImagodError>>>,
    heartbeat_task: Option<JoinHandle<()>>,
    run_task: Option<JoinHandle<Result<(), ImagodError>>>,
) -> Option<Result<(), ImagodError>> {
    let _ = shutdown_tx.send(true);
    if let Some(task) = heartbeat_task {
        let _ = task.await;
    }
    if let Some(task) = run_task {
        let _ = task.await;
    }
    let http_result = match http_ingress_task {
        Some(task) => Some(
            task.await
                .map_err(map_http_ingress_join_error)
                .and_then(|v| v),
        ),
        None => None,
    };
    let _ = inbound_task.await;
    http_result
}

/// Converts spawned run task join failures to a unified error.
fn map_run_join_error(err: tokio::task::JoinError) -> ImagodError {
    ImagodError::new(
        ErrorCode::Internal,
        STAGE_RUNNER,
        format!("runner run task join failed: {err}"),
    )
}

fn map_http_ingress_join_error(err: tokio::task::JoinError) -> ImagodError {
    ImagodError::new(
        ErrorCode::Internal,
        STAGE_HTTP_INGRESS,
        format!("http ingress task join failed: {err}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[cfg(not(feature = "runtime-wasmtime"))]
    #[test]
    fn runtime_backend_disabled_returns_explicit_error() {
        let err = match create_runtime_backend(&NativePluginRegistry::default()) {
            Ok(_) => panic!("backend creation should fail when runtime-wasmtime is disabled"),
            Err(err) => err,
        };
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, STAGE_RUNNER);
        assert!(
            err.message.contains("runtime backend is not enabled"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn startup_window_detects_early_error_exit() {
        let mut run_task = tokio::spawn(async {
            Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNNER,
                "early startup failure",
            ))
        });
        let state = observe_startup_window(&mut run_task, STARTUP_CONFIRM_WINDOW)
            .await
            .expect("startup observation should succeed");
        match state {
            StartupRunState::Finished(Err(err)) => assert_eq!(err.code, ErrorCode::Internal),
            _ => panic!("startup should classify early error as finished failure"),
        }
    }

    #[tokio::test]
    async fn startup_window_keeps_early_ok_exit_compatible() {
        let mut run_task = tokio::spawn(async { Ok(()) });
        let state = observe_startup_window(&mut run_task, STARTUP_CONFIRM_WINDOW)
            .await
            .expect("startup observation should succeed");
        match state {
            StartupRunState::Finished(Ok(())) => {}
            _ => panic!("startup should classify early ok as finished success"),
        }
    }

    #[tokio::test]
    async fn startup_window_recognizes_running_task_after_window() {
        let mut run_task = tokio::spawn(async {
            time::sleep(Duration::from_millis(80)).await;
            Ok(())
        });
        let state = observe_startup_window(&mut run_task, Duration::from_millis(10))
            .await
            .expect("startup observation should succeed");
        assert!(matches!(state, StartupRunState::StillRunning));
        join_run_task(run_task)
            .await
            .expect("run task should complete successfully");
    }

    #[tokio::test]
    async fn http_runtime_ready_wait_returns_ready_when_signal_arrives_first() {
        let (ready_tx, ready_rx) = oneshot::channel::<()>();
        let mut run_task = tokio::spawn(async {
            time::sleep(Duration::from_millis(80)).await;
            Ok(())
        });
        ready_tx
            .send(())
            .expect("ready signal should be delivered to waiter");

        let state = wait_http_runtime_ready_or_exit(&mut run_task, ready_rx)
            .await
            .expect("ready observation should succeed");
        assert!(matches!(state, HttpRuntimeReadyState::Ready));
        join_run_task(run_task)
            .await
            .expect("run task should complete successfully");
    }

    #[tokio::test]
    async fn http_runtime_ready_wait_returns_finished_when_run_exits_first() {
        let (_ready_tx, ready_rx) = oneshot::channel::<()>();
        let mut run_task = tokio::spawn(async {
            Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNNER,
                "http startup failed",
            ))
        });

        let state = wait_http_runtime_ready_or_exit(&mut run_task, ready_rx)
            .await
            .expect("ready observation should succeed");
        match state {
            HttpRuntimeReadyState::Finished(Err(err)) => {
                assert_eq!(err.code, ErrorCode::Internal)
            }
            _ => panic!("run task failure should win over ready signal"),
        }
    }
}
