//! Runner process bootstrap and inbound control loop.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming as HyperIncomingBody;
use hyper::{Request, Response, server::conn::http1, service::service_fn};
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{
    ControlRequest, ControlResponse, IpcErrorPayload, RunnerAppType, RunnerBootstrap,
    RunnerInboundRequest, RunnerInboundResponse, compute_manager_auth_proof,
    dbus_p2p::DbusP2pTransport, verify_invocation_token, verify_manager_auth_proof,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::{TcpListener, UnixListener, UnixStream},
    sync::{Semaphore, watch},
    task::JoinHandle,
    time::{self, Duration},
};
use wasmtime_wasi_http::io::TokioIo;

use crate::{
    runtime::{ComponentRuntime, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeRunRequest},
    runtime_wasmtime::WasmRuntime,
};

const STAGE_RUNNER: &str = "runner.process";
const STAGE_SHUTDOWN: &str = "runner.shutdown";
const STAGE_INBOUND: &str = "runner.inbound";
const STAGE_HTTP_INGRESS: &str = "runner.http_ingress";
const INBOUND_READ_TIMEOUT_SECS: u64 = 3;
const INBOUND_ACCEPT_RETRY_BACKOFF_MS: u64 = 25;
const HEARTBEAT_RPC_TIMEOUT_SECS: u64 = 2;
const MAX_INBOUND_CONNECTION_HANDLERS: usize = 32;
const MAX_RUNNER_BOOTSTRAP_BYTES: usize = 64 * 1024;
const MAX_CONSECUTIVE_HEARTBEAT_FAILURES: u32 = 3;
const STARTUP_CONFIRM_WINDOW: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeartbeatDecision {
    Continue,
    Shutdown,
}

/// Startup observation result for runner workload task.
enum StartupRunState {
    /// Workload is still running after startup confirmation window.
    StillRunning,
    /// Workload exited during startup confirmation window.
    Finished(Result<(), ImagodError>),
}

/// Ensures runner endpoint socket path is removed when function scope exits.
struct SocketCleanupGuard {
    path: PathBuf,
}

impl SocketCleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for SocketCleanupGuard {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                eprintln!(
                    "failed to remove runner endpoint {}: {err}",
                    self.path.display()
                );
            }
            Err(_) => {}
        }
    }
}

/// Starts runner mode by reading `RunnerBootstrap` from stdin and executing the component.
///
/// The function registers the runner with manager, serves inbound IPC requests,
/// emits heartbeat signals, and exits when the component finishes or shutdown is requested.
pub async fn run_runner_from_stdin() -> Result<(), ImagodError> {
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

    let runtime: Arc<dyn ComponentRuntime> = Arc::new(WasmRuntime::new()?);
    runtime.validate_component(&bootstrap.component_path)?;

    register_runner(&bootstrap).await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let inbound_task = tokio::spawn(run_inbound_server(
        listener,
        bootstrap.clone(),
        shutdown_tx.clone(),
        shutdown_rx.clone(),
    ));
    let http_ingress_task = match bootstrap.app_type {
        RunnerAppType::Http => Some(
            spawn_http_ingress_server(
                runtime.clone(),
                bootstrap.clone(),
                shutdown_tx.clone(),
                shutdown_tx.subscribe(),
            )
            .await?,
        ),
        RunnerAppType::Cli | RunnerAppType::Socket => None,
    };

    let runtime_for_run = runtime.clone();
    let bootstrap_for_run = bootstrap.clone();
    let mut run_task = tokio::spawn(async move {
        runtime_for_run
            .run_component(RuntimeRunRequest {
                app_type: bootstrap_for_run.app_type,
                component_path: bootstrap_for_run.component_path.clone(),
                args: bootstrap_for_run.args.clone(),
                envs: bootstrap_for_run.envs.clone(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: bootstrap_for_run.epoch_tick_interval_ms,
            })
            .await
    });

    match observe_startup_window(&mut run_task, STARTUP_CONFIRM_WINDOW).await? {
        StartupRunState::Finished(run_result) => {
            if run_result.is_ok()
                && let Err(err) = mark_ready(&bootstrap).await
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
            let http_result =
                shutdown_runner_tasks(&shutdown_tx, inbound_task, http_ingress_task, None, None)
                    .await;
            if let Some(Err(err)) = http_result {
                return Err(err);
            }
            return run_result;
        }
        StartupRunState::StillRunning => {}
    }

    if let Err(err) = mark_ready(&bootstrap).await {
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

    let heartbeat_task = tokio::spawn(send_heartbeats(
        bootstrap.clone(),
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

async fn spawn_http_ingress_server(
    runtime: Arc<dyn ComponentRuntime>,
    bootstrap: RunnerBootstrap,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<JoinHandle<Result<(), ImagodError>>, ImagodError> {
    let port = required_http_port(&bootstrap)?;
    let bind_addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_HTTP_INGRESS,
            format!("failed to bind http ingress {bind_addr}: {e}"),
        )
    })?;

    Ok(tokio::spawn(run_http_ingress_server(
        listener,
        runtime,
        bootstrap,
        shutdown_tx,
        shutdown_rx,
    )))
}

fn required_http_port(bootstrap: &RunnerBootstrap) -> Result<u16, ImagodError> {
    match bootstrap.http_port {
        Some(port) if port > 0 => Ok(port),
        _ => Err(ImagodError::new(
            ErrorCode::Internal,
            STAGE_HTTP_INGRESS,
            "type=http requires http_port in runner bootstrap",
        )),
    }
}

async fn run_http_ingress_server(
    listener: TcpListener,
    runtime: Arc<dyn ComponentRuntime>,
    bootstrap: RunnerBootstrap,
    shutdown_tx: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ImagodError> {
    let concurrency = Arc::new(Semaphore::new(MAX_INBOUND_CONNECTION_HANDLERS));
    let mut connection_tasks = tokio::task::JoinSet::new();

    loop {
        let permit = tokio::select! {
            acquired = concurrency.clone().acquire_owned() => {
                match acquired {
                    Ok(permit) => permit,
                    Err(_) => break,
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        };

        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            changed = shutdown_rx.changed() => {
                drop(permit);
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        };

        let (stream, _) = match accepted {
            Ok(v) => v,
            Err(err) => {
                drop(permit);
                if should_retry_inbound_accept(&err) {
                    eprintln!("runner http ingress accept transient error: {err}");
                    time::sleep(Duration::from_millis(INBOUND_ACCEPT_RETRY_BACKOFF_MS)).await;
                    continue;
                }
                let _ = shutdown_tx.send(true);
                return Err(ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_HTTP_INGRESS,
                    format!(
                        "http ingress accept failed for service '{}': {err}",
                        bootstrap.service_name
                    ),
                ));
            }
        };

        let runtime = runtime.clone();
        let service_name = bootstrap.service_name.clone();
        connection_tasks.spawn(async move {
            let _permit = permit;
            if let Err(err) = serve_http_connection(stream, runtime).await {
                eprintln!(
                    "runner http ingress connection error service={} error={}",
                    service_name, err
                );
            }
        });
    }

    connection_tasks.abort_all();
    while connection_tasks.join_next().await.is_some() {}
    Ok(())
}

async fn serve_http_connection(
    stream: tokio::net::TcpStream,
    runtime: Arc<dyn ComponentRuntime>,
) -> Result<(), ImagodError> {
    let service = service_fn(move |request: Request<HyperIncomingBody>| {
        let runtime = runtime.clone();
        async move { Ok::<_, std::convert::Infallible>(handle_http_request(runtime, request).await) }
    });

    http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_HTTP_INGRESS,
                format!("failed to serve http connection: {e}"),
            )
        })
}

async fn handle_http_request(
    runtime: Arc<dyn ComponentRuntime>,
    request: Request<HyperIncomingBody>,
) -> Response<Full<Bytes>> {
    let request = match into_runtime_http_request(request).await {
        Ok(v) => v,
        Err(err) => return runtime_error_response(err),
    };

    match runtime.handle_http_request(request).await {
        Ok(response) => runtime_http_response_to_hyper(response),
        Err(err) => runtime_error_response(err),
    }
}

async fn into_runtime_http_request(
    request: Request<HyperIncomingBody>,
) -> Result<RuntimeHttpRequest, ImagodError> {
    let (parts, body) = request.into_parts();
    let body = BodyExt::collect(body).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_HTTP_INGRESS,
            format!("failed to read request body: {e}"),
        )
    })?;

    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();

    Ok(RuntimeHttpRequest {
        method: parts.method.as_str().to_string(),
        uri: parts.uri.to_string(),
        headers,
        body: body.to_bytes().to_vec(),
    })
}

fn runtime_http_response_to_hyper(response: RuntimeHttpResponse) -> Response<Full<Bytes>> {
    let status = hyper::StatusCode::from_u16(response.status)
        .unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);

    let mut out = Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(response.body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"invalid response"))));
    for (name, value) in response.headers {
        let Ok(name) = hyper::header::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = hyper::header::HeaderValue::from_bytes(&value) else {
            continue;
        };
        out.headers_mut().append(name, value);
    }
    out
}

fn runtime_error_response(error: ImagodError) -> Response<Full<Bytes>> {
    let body = format!("runtime error: {} ({})", error.message, error.stage);
    Response::builder()
        .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"runtime error"))))
}

async fn read_runner_bootstrap<R>(reader: R) -> Result<RunnerBootstrap, ImagodError>
where
    R: AsyncRead + Unpin,
{
    let mut limited_reader = reader.take((MAX_RUNNER_BOOTSTRAP_BYTES + 1) as u64);
    let mut bootstrap_bytes = Vec::new();
    limited_reader
        .read_to_end(&mut bootstrap_bytes)
        .await
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_RUNNER,
                format!("failed to read runner bootstrap from stdin: {e}"),
            )
        })?;
    decode_runner_bootstrap(&bootstrap_bytes)
}

fn decode_runner_bootstrap(bootstrap_bytes: &[u8]) -> Result<RunnerBootstrap, ImagodError> {
    validate_runner_bootstrap_size(bootstrap_bytes.len())?;
    imago_protocol::from_cbor::<RunnerBootstrap>(bootstrap_bytes).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNNER,
            format!("failed to decode runner bootstrap: {e}"),
        )
    })
}

fn validate_runner_bootstrap_size(bootstrap_len: usize) -> Result<(), ImagodError> {
    if bootstrap_len > MAX_RUNNER_BOOTSTRAP_BYTES {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNNER,
            format!(
                "runner bootstrap is too large: {bootstrap_len} bytes (max {MAX_RUNNER_BOOTSTRAP_BYTES})"
            ),
        ));
    }
    Ok(())
}

/// Registers this runner endpoint with the manager control plane.
async fn register_runner(bootstrap: &RunnerBootstrap) -> Result<(), ImagodError> {
    let proof = compute_manager_auth_proof(&bootstrap.manager_auth_secret, &bootstrap.runner_id)?;
    let response = DbusP2pTransport::call_control(
        &bootstrap.manager_control_endpoint,
        &ControlRequest::RegisterRunner {
            runner_id: bootstrap.runner_id.clone(),
            service_name: bootstrap.service_name.clone(),
            release_hash: bootstrap.release_hash.clone(),
            runner_endpoint: bootstrap.runner_endpoint.clone(),
            manager_auth_proof: proof,
        },
    )
    .await?;
    match response {
        ControlResponse::Ack => Ok(()),
        ControlResponse::Error(err) => Err(err.to_error()),
        _ => Err(ImagodError::new(
            ErrorCode::Internal,
            STAGE_RUNNER,
            "unexpected manager response for register_runner",
        )),
    }
}

/// Notifies manager that runner initialization has completed.
async fn mark_ready(bootstrap: &RunnerBootstrap) -> Result<(), ImagodError> {
    let proof = compute_manager_auth_proof(&bootstrap.manager_auth_secret, &bootstrap.runner_id)?;
    let response = DbusP2pTransport::call_control(
        &bootstrap.manager_control_endpoint,
        &ControlRequest::RunnerReady {
            runner_id: bootstrap.runner_id.clone(),
            manager_auth_proof: proof,
        },
    )
    .await?;

    match response {
        ControlResponse::Ack => Ok(()),
        ControlResponse::Error(err) => Err(err.to_error()),
        _ => Err(ImagodError::new(
            ErrorCode::Internal,
            STAGE_RUNNER,
            "unexpected manager response for runner_ready",
        )),
    }
}

/// Sends periodic heartbeat messages until shutdown is requested.
///
/// Concurrency: runs as a dedicated background task.
async fn send_heartbeats(
    bootstrap: RunnerBootstrap,
    shutdown_tx: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut consecutive_failures = 0u32;
    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        let proof = match compute_manager_auth_proof(
            &bootstrap.manager_auth_secret,
            &bootstrap.runner_id,
        ) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("runner heartbeat auth error: {err}");
                let _ = shutdown_tx.send(true);
                break;
            }
        };
        let response = match time::timeout(
            Duration::from_secs(HEARTBEAT_RPC_TIMEOUT_SECS),
            DbusP2pTransport::call_control(
                &bootstrap.manager_control_endpoint,
                &ControlRequest::Heartbeat {
                    runner_id: bootstrap.runner_id.clone(),
                    manager_auth_proof: proof,
                },
            ),
        )
        .await
        {
            Ok(response) => response,
            Err(_) => Err(ImagodError::new(
                ErrorCode::OperationTimeout,
                STAGE_RUNNER,
                format!(
                    "runner heartbeat request timed out after {HEARTBEAT_RPC_TIMEOUT_SECS} seconds"
                ),
            )),
        };
        if evaluate_heartbeat_result(response, &mut consecutive_failures)
            == HeartbeatDecision::Shutdown
        {
            let _ = shutdown_tx.send(true);
            break;
        }

        tokio::select! {
            _ = time::sleep(Duration::from_secs(1)) => {}
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }
}

fn evaluate_heartbeat_result(
    result: Result<ControlResponse, ImagodError>,
    consecutive_failures: &mut u32,
) -> HeartbeatDecision {
    match result {
        Ok(ControlResponse::Ack) => {
            *consecutive_failures = 0;
            HeartbeatDecision::Continue
        }
        Ok(ControlResponse::Error(err))
            if err.code == ErrorCode::NotFound || err.code == ErrorCode::Unauthorized =>
        {
            eprintln!(
                "runner heartbeat rejected by manager: code={:?} stage={} message={}",
                err.code, err.stage, err.message
            );
            HeartbeatDecision::Shutdown
        }
        Ok(ControlResponse::Error(err)) => {
            eprintln!(
                "runner heartbeat error response: code={:?} stage={} message={}",
                err.code, err.stage, err.message
            );
            apply_retryable_heartbeat_failure(consecutive_failures)
        }
        Ok(other) => {
            eprintln!("runner heartbeat unexpected response: {other:?}");
            apply_retryable_heartbeat_failure(consecutive_failures)
        }
        Err(err) => {
            eprintln!("runner heartbeat transport error: {err}");
            apply_retryable_heartbeat_failure(consecutive_failures)
        }
    }
}

fn apply_retryable_heartbeat_failure(consecutive_failures: &mut u32) -> HeartbeatDecision {
    *consecutive_failures += 1;
    if *consecutive_failures >= MAX_CONSECUTIVE_HEARTBEAT_FAILURES {
        eprintln!(
            "runner heartbeat failed {} consecutive times; requesting shutdown",
            consecutive_failures
        );
        HeartbeatDecision::Shutdown
    } else {
        HeartbeatDecision::Continue
    }
}

/// Accepts inbound runner requests and writes one response per request.
///
/// Concurrency: runs as a dedicated background task.
async fn run_inbound_server(
    listener: UnixListener,
    bootstrap: RunnerBootstrap,
    shutdown_tx: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let concurrency = Arc::new(Semaphore::new(MAX_INBOUND_CONNECTION_HANDLERS));
    loop {
        let permit = tokio::select! {
            acquired = concurrency.clone().acquire_owned() => {
                match acquired {
                    Ok(permit) => permit,
                    Err(_) => break,
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        };

        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            changed = shutdown_rx.changed() => {
                drop(permit);
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        };

        let (mut stream, _) = match accepted {
            Ok(v) => v,
            Err(err) => {
                drop(permit);
                if should_retry_inbound_accept(&err) {
                    eprintln!("runner inbound accept transient error: {err}");
                    time::sleep(Duration::from_millis(INBOUND_ACCEPT_RETRY_BACKOFF_MS)).await;
                    continue;
                }
                eprintln!("runner inbound accept error: {err}");
                break;
            }
        };

        let bootstrap = bootstrap.clone();
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            let _permit = permit;
            handle_inbound_connection(&mut stream, bootstrap, shutdown_tx).await;
        });
    }
}

fn should_retry_inbound_accept(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::TimedOut
    )
}

async fn handle_inbound_connection(
    stream: &mut UnixStream,
    bootstrap: RunnerBootstrap,
    shutdown_tx: watch::Sender<bool>,
) {
    let request = match time::timeout(
        Duration::from_secs(INBOUND_READ_TIMEOUT_SECS),
        DbusP2pTransport::read_message::<RunnerInboundRequest>(stream),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(err)) => {
            let _ = DbusP2pTransport::write_message(
                stream,
                &RunnerInboundResponse::Error(IpcErrorPayload::from_error(&err)),
            )
            .await;
            return;
        }
        Err(_) => {
            let _ = DbusP2pTransport::write_message(
                stream,
                &RunnerInboundResponse::Error(IpcErrorPayload::from_error(&ImagodError::new(
                    ErrorCode::OperationTimeout,
                    STAGE_INBOUND,
                    "runner inbound request timed out",
                ))),
            )
            .await;
            return;
        }
    };

    let response = handle_inbound_request(&bootstrap, request, &shutdown_tx).await;
    let _ = DbusP2pTransport::write_message(stream, &response).await;
}

fn validate_shutdown_auth(
    manager_auth_secret: &str,
    runner_id: &str,
    manager_auth_proof: &str,
) -> Result<(), ImagodError> {
    match verify_manager_auth_proof(manager_auth_secret, runner_id, manager_auth_proof) {
        Ok(()) => Ok(()),
        Err(err) if err.code == ErrorCode::Unauthorized => Err(ImagodError::new(
            ErrorCode::Unauthorized,
            STAGE_SHUTDOWN,
            "manager auth proof mismatch",
        )),
        Err(err) => Err(ImagodError::new(
            err.code,
            STAGE_SHUTDOWN,
            format!("manager auth proof verification failed: {}", err.message),
        )),
    }
}

/// Handles a single inbound request and performs token validation for invoke calls.
async fn handle_inbound_request(
    bootstrap: &RunnerBootstrap,
    request: RunnerInboundRequest,
    shutdown_tx: &watch::Sender<bool>,
) -> RunnerInboundResponse {
    match request {
        RunnerInboundRequest::ShutdownRunner { manager_auth_proof } => {
            if let Err(err) = validate_shutdown_auth(
                &bootstrap.manager_auth_secret,
                &bootstrap.runner_id,
                &manager_auth_proof,
            ) {
                return RunnerInboundResponse::Error(IpcErrorPayload::from_error(&err));
            }
            let _ = shutdown_tx.send(true);
            RunnerInboundResponse::Ack
        }
        RunnerInboundRequest::Invoke {
            interface_id,
            function,
            payload_cbor: _,
            token,
        } => {
            let claims = match verify_invocation_token(&bootstrap.invocation_secret, &token) {
                Ok(claims) => claims,
                Err(err) => {
                    return RunnerInboundResponse::Error(IpcErrorPayload::from_error(&err));
                }
            };

            if claims.target_service != bootstrap.service_name {
                return RunnerInboundResponse::Error(IpcErrorPayload {
                    code: ErrorCode::Unauthorized,
                    stage: "runner.invoke".to_string(),
                    message: "invocation target mismatch".to_string(),
                });
            }

            if claims.wit != interface_id {
                return RunnerInboundResponse::Error(IpcErrorPayload {
                    code: ErrorCode::Unauthorized,
                    stage: "runner.invoke".to_string(),
                    message: "invocation interface mismatch".to_string(),
                });
            }

            RunnerInboundResponse::Error(IpcErrorPayload {
                code: ErrorCode::Internal,
                stage: "runner.invoke".to_string(),
                message: format!(
                    "invoke is not implemented yet (interface={}, function={})",
                    interface_id, function
                ),
            })
        }
    }
}

/// Ensures runner socket parent exists and removes stale socket files before bind.
fn prepare_socket_path(path: &Path) -> Result<(), ImagodError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNNER,
                format!(
                    "failed to create runner socket parent {}: {e}",
                    parent.display()
                ),
            )
        })?;
    }

    if path.exists() {
        std::fs::remove_file(path).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNNER,
                format!("failed to remove existing socket {}: {e}", path.display()),
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        RuntimeHttpFuture, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeRunFuture,
        RuntimeRunRequest,
    };
    use std::{
        collections::BTreeMap,
        io::Cursor,
        os::unix::net::UnixListener as StdUnixListener,
        path::{Path, PathBuf},
        sync::Mutex as StdMutex,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    use imagod_ipc::{RunnerAppType, RunnerInboundResponse, random_secret_hex};

    fn run_async_test<F>(future: F)
    where
        F: std::future::Future<Output = ()>,
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(future);
    }

    fn new_test_root(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        PathBuf::from(format!("/tmp/iss-runner-{prefix}-{ts}"))
    }

    fn new_test_socket_path(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        let root = PathBuf::from(format!(
            "/tmp/imago-runtime-runner-test-{prefix}-{}-{ts}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("test root should be created");
        root.join("runner.sock")
    }

    fn new_test_bootstrap(root: &Path, runner_socket_name: &str) -> RunnerBootstrap {
        let runner_id = "runner-1".to_string();
        RunnerBootstrap {
            runner_id: runner_id.clone(),
            service_name: "svc-test".to_string(),
            release_hash: "release-test".to_string(),
            app_type: RunnerAppType::Cli,
            http_port: None,
            component_path: root.join("component.wasm"),
            args: Vec::new(),
            envs: BTreeMap::new(),
            bindings: Vec::new(),
            manager_control_endpoint: root.join("manager-control.sock"),
            runner_endpoint: root.join(runner_socket_name),
            manager_auth_secret: random_secret_hex(),
            invocation_secret: random_secret_hex(),
            epoch_tick_interval_ms: 50,
        }
    }

    fn reserve_test_http_port() -> u16 {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral listener should bind");
        let port = listener
            .local_addr()
            .expect("local_addr should be available")
            .port();
        drop(listener);
        port
    }

    #[derive(Default)]
    struct MockHttpRuntime {
        calls: AtomicUsize,
        last_path: StdMutex<Option<String>>,
    }

    impl ComponentRuntime for MockHttpRuntime {
        fn validate_component(&self, _component_path: &Path) -> Result<(), ImagodError> {
            Ok(())
        }

        fn run_component<'a>(&'a self, _request: RuntimeRunRequest) -> RuntimeRunFuture<'a> {
            Box::pin(async { Ok(()) })
        }

        fn handle_http_request<'a>(&'a self, request: RuntimeHttpRequest) -> RuntimeHttpFuture<'a> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                *self.last_path.lock().expect("mutex should lock") = Some(request.uri.clone());
                Ok(RuntimeHttpResponse {
                    status: 200,
                    headers: vec![("content-type".to_string(), b"text/plain".to_vec())],
                    body: b"hello-http".to_vec(),
                })
            })
        }
    }

    #[tokio::test]
    async fn http_ingress_requires_http_port_for_http_type() {
        let root = new_test_root("http-port-required");
        let mut bootstrap = new_test_bootstrap(&root, "runner.sock");
        bootstrap.app_type = RunnerAppType::Http;
        bootstrap.http_port = None;

        let runtime: Arc<dyn ComponentRuntime> = Arc::new(MockHttpRuntime::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let err = spawn_http_ingress_server(runtime, bootstrap, shutdown_tx, shutdown_rx)
            .await
            .expect_err("http ingress should reject missing http_port");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.message.contains("requires http_port"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn http_ingress_handles_request_and_shutdown() {
        let root = new_test_root("http-ingress-request");
        let mut bootstrap = new_test_bootstrap(&root, "runner.sock");
        bootstrap.app_type = RunnerAppType::Http;
        let port = reserve_test_http_port();
        bootstrap.http_port = Some(port);

        let runtime = Arc::new(MockHttpRuntime::default());
        let runtime_trait: Arc<dyn ComponentRuntime> = runtime.clone();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let ingress_task = spawn_http_ingress_server(
            runtime_trait,
            bootstrap.clone(),
            shutdown_tx.clone(),
            shutdown_rx,
        )
        .await
        .expect("http ingress should start");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("client should connect");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("request write should succeed");

        let mut response_bytes = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(2),
            stream.read_to_end(&mut response_bytes),
        )
        .await
        .expect("response read should complete")
        .expect("response read should succeed");
        let response_text = String::from_utf8_lossy(&response_bytes);
        assert!(
            response_text.starts_with("HTTP/1.1 200"),
            "unexpected status line: {response_text}"
        );
        assert!(response_text.contains("hello-http"));

        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime
                .last_path
                .lock()
                .expect("mutex should lock")
                .as_deref(),
            Some("/health")
        );

        let _ = shutdown_tx.send(true);
        let joined = tokio::time::timeout(Duration::from_secs(2), ingress_task)
            .await
            .expect("ingress task should stop after shutdown")
            .expect("ingress task join should succeed");
        assert!(joined.is_ok(), "ingress should stop cleanly");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn shutdown_runner_rejects_invalid_manager_auth_proof() {
        let root = new_test_root("shutdown-auth");
        let bootstrap = new_test_bootstrap(&root, "runner.sock");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let response = handle_inbound_request(
            &bootstrap,
            RunnerInboundRequest::ShutdownRunner {
                manager_auth_proof: "invalid-proof".to_string(),
            },
            &shutdown_tx,
        )
        .await;

        match response {
            RunnerInboundResponse::Error(err) => assert_eq!(err.code, ErrorCode::Unauthorized),
            other => panic!("unexpected response: {other:?}"),
        }
        assert!(
            !*shutdown_rx.borrow(),
            "shutdown signal should not be accepted without valid auth"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn shutdown_runner_accepts_valid_manager_auth_proof() {
        let root = new_test_root("shutdown-auth-valid");
        let bootstrap = new_test_bootstrap(&root, "runner.sock");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let manager_auth_proof =
            compute_manager_auth_proof(&bootstrap.manager_auth_secret, &bootstrap.runner_id)
                .expect("manager proof should compute");
        let response = handle_inbound_request(
            &bootstrap,
            RunnerInboundRequest::ShutdownRunner { manager_auth_proof },
            &shutdown_tx,
        )
        .await;

        assert!(
            matches!(response, RunnerInboundResponse::Ack),
            "valid auth should be accepted"
        );
        assert!(*shutdown_rx.borrow(), "shutdown signal should be requested");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn inbound_server_keeps_accepting_when_first_connection_stalls() {
        let root = new_test_root("inbound-hol");
        let bootstrap = new_test_bootstrap(&root, "runner.sock");
        prepare_socket_path(&bootstrap.runner_endpoint).expect("socket path should prepare");
        let listener =
            UnixListener::bind(&bootstrap.runner_endpoint).expect("runner listener should bind");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut shutdown_observer = shutdown_tx.subscribe();
        let server_task = tokio::spawn(run_inbound_server(
            listener,
            bootstrap.clone(),
            shutdown_tx.clone(),
            shutdown_rx,
        ));

        let idle = tokio::net::UnixStream::connect(&bootstrap.runner_endpoint)
            .await
            .expect("idle connection should open");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let manager_auth_proof =
            compute_manager_auth_proof(&bootstrap.manager_auth_secret, &bootstrap.runner_id)
                .expect("manager proof should compute");
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            DbusP2pTransport::call_runner(
                &bootstrap.runner_endpoint,
                &RunnerInboundRequest::ShutdownRunner { manager_auth_proof },
            ),
        )
        .await
        .expect("shutdown request should not be blocked by idle peer")
        .expect("shutdown request should return response");
        assert!(matches!(response, RunnerInboundResponse::Ack));

        tokio::time::timeout(Duration::from_secs(2), shutdown_observer.changed())
            .await
            .expect("shutdown signal should be observed")
            .expect("shutdown observer should remain active");
        assert!(*shutdown_observer.borrow(), "shutdown should become true");

        drop(idle);
        tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task should exit after shutdown")
            .expect("server task should join cleanly");

        let _ = std::fs::remove_file(&bootstrap.runner_endpoint);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn heartbeat_task_can_shutdown_while_manager_never_responds() {
        let root = new_test_root("heartbeat-timeout");
        let bootstrap = new_test_bootstrap(&root, "runner-timeout.sock");
        if let Some(parent) = bootstrap.manager_control_endpoint.parent() {
            std::fs::create_dir_all(parent).expect("manager control parent should be created");
        }
        let _ = std::fs::remove_file(&bootstrap.manager_control_endpoint);
        let listener =
            UnixListener::bind(&bootstrap.manager_control_endpoint).expect("listener should bind");

        let (accepted_tx, accepted_rx) = oneshot::channel::<()>();
        let server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept should succeed");
            let _ = accepted_tx.send(());
            let _ = DbusP2pTransport::read_message::<ControlRequest>(&mut stream).await;
            tokio::time::sleep(Duration::from_secs(10)).await;
        });

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let heartbeat_task = tokio::spawn(send_heartbeats(
            bootstrap.clone(),
            shutdown_tx.clone(),
            shutdown_rx,
        ));

        tokio::time::timeout(Duration::from_secs(2), accepted_rx)
            .await
            .expect("heartbeat should connect manager socket")
            .expect("accept signal should be sent");

        let _ = shutdown_tx.send(true);
        tokio::time::timeout(
            Duration::from_secs(HEARTBEAT_RPC_TIMEOUT_SECS + 2),
            heartbeat_task,
        )
        .await
        .expect("heartbeat task should finish after timeout")
        .expect("heartbeat task should join");

        server_task.abort();
        let _ = std::fs::remove_file(&bootstrap.manager_control_endpoint);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn startup_window_detects_early_error_exit() {
        run_async_test(async {
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
        });
    }

    #[test]
    fn startup_window_keeps_early_ok_exit_compatible() {
        run_async_test(async {
            let mut run_task = tokio::spawn(async { Ok(()) });
            let state = observe_startup_window(&mut run_task, STARTUP_CONFIRM_WINDOW)
                .await
                .expect("startup observation should succeed");
            match state {
                StartupRunState::Finished(Ok(())) => {}
                _ => panic!("startup should classify early ok as finished success"),
            }
        });
    }

    #[test]
    fn startup_window_recognizes_running_task_after_window() {
        run_async_test(async {
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
        });
    }

    #[test]
    fn socket_cleanup_guard_removes_endpoint_on_drop() {
        let socket_path = new_test_socket_path("cleanup");
        let parent = socket_path
            .parent()
            .expect("socket parent should exist")
            .to_path_buf();
        prepare_socket_path(&socket_path).expect("socket parent preparation should succeed");

        let listener = StdUnixListener::bind(&socket_path).expect("socket bind should succeed");
        assert!(socket_path.exists());
        {
            let _cleanup_guard = SocketCleanupGuard::new(socket_path.clone());
        }
        assert!(
            !socket_path.exists(),
            "socket path should be removed by cleanup guard"
        );

        drop(listener);
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn should_retry_inbound_accept_true_for_transient_kinds() {
        let transient = [
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::TimedOut,
        ];

        for kind in transient {
            let err = std::io::Error::new(kind, "transient");
            assert!(
                should_retry_inbound_accept(&err),
                "kind {:?} should be retried",
                kind
            );
        }
    }

    #[test]
    fn should_retry_inbound_accept_false_for_non_transient_kinds() {
        let non_transient = [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidInput,
            std::io::ErrorKind::AddrInUse,
        ];

        for kind in non_transient {
            let err = std::io::Error::new(kind, "fatal");
            assert!(
                !should_retry_inbound_accept(&err),
                "kind {:?} should not be retried",
                kind
            );
        }
    }

    #[test]
    fn validate_runner_bootstrap_size_accepts_exact_limit() {
        assert!(validate_runner_bootstrap_size(MAX_RUNNER_BOOTSTRAP_BYTES).is_ok());
    }

    #[test]
    fn validate_runner_bootstrap_size_rejects_over_limit() {
        let err = validate_runner_bootstrap_size(MAX_RUNNER_BOOTSTRAP_BYTES + 1)
            .expect_err("oversized bootstrap should be rejected");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.message.contains("too large"));
    }

    #[test]
    fn read_runner_bootstrap_rejects_oversized_input_before_decode() {
        run_async_test(async {
            let oversized = vec![0u8; MAX_RUNNER_BOOTSTRAP_BYTES + 1];
            let err = read_runner_bootstrap(Cursor::new(oversized))
                .await
                .expect_err("oversized bootstrap should fail before decode");
            assert_eq!(err.code, ErrorCode::BadRequest);
            assert!(err.message.contains("too large"));
        });
    }

    #[test]
    fn heartbeat_result_shutdowns_immediately_for_not_found_and_unauthorized() {
        let mut failures = 0;
        let not_found = evaluate_heartbeat_result(
            Ok(ControlResponse::Error(IpcErrorPayload {
                code: ErrorCode::NotFound,
                stage: STAGE_RUNNER.to_string(),
                message: "runner missing".to_string(),
            })),
            &mut failures,
        );
        assert_eq!(not_found, HeartbeatDecision::Shutdown);
        assert_eq!(failures, 0);

        let unauthorized = evaluate_heartbeat_result(
            Ok(ControlResponse::Error(IpcErrorPayload {
                code: ErrorCode::Unauthorized,
                stage: STAGE_RUNNER.to_string(),
                message: "auth failed".to_string(),
            })),
            &mut failures,
        );
        assert_eq!(unauthorized, HeartbeatDecision::Shutdown);
        assert_eq!(failures, 0);
    }

    #[test]
    fn heartbeat_result_shutdowns_after_three_retryable_failures() {
        let mut failures = 0;
        for _ in 0..(MAX_CONSECUTIVE_HEARTBEAT_FAILURES - 1) {
            let decision = evaluate_heartbeat_result(
                Ok(ControlResponse::Error(IpcErrorPayload {
                    code: ErrorCode::OperationTimeout,
                    stage: STAGE_RUNNER.to_string(),
                    message: "timeout".to_string(),
                })),
                &mut failures,
            );
            assert_eq!(decision, HeartbeatDecision::Continue);
        }

        let decision = evaluate_heartbeat_result(
            Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNNER,
                "transport down",
            )),
            &mut failures,
        );
        assert_eq!(decision, HeartbeatDecision::Shutdown);
        assert_eq!(failures, MAX_CONSECUTIVE_HEARTBEAT_FAILURES);
    }

    #[test]
    fn heartbeat_result_ack_resets_failure_counter() {
        let mut failures = 2;
        let decision = evaluate_heartbeat_result(Ok(ControlResponse::Ack), &mut failures);
        assert_eq!(decision, HeartbeatDecision::Continue);
        assert_eq!(failures, 0);
    }
}
