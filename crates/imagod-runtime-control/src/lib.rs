//! Runner control-plane coordination utilities (manager client, inbound loop, heartbeat loop).

use std::sync::Arc;

use async_trait::async_trait;
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{
    ControlRequest, ControlResponse, IpcErrorPayload, RunnerBootstrap, RunnerInboundRequest,
    RunnerInboundResponse, compute_manager_auth_proof, dbus_p2p::DbusP2pTransport,
    verify_invocation_token, verify_manager_auth_proof,
};
use imagod_runtime_internal::{RuntimeInvokeRequest, RuntimeInvoker};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::{Semaphore, watch},
    time::{self, Duration},
};

pub const STAGE_RUNNER: &str = "runner.process";
pub const STAGE_SHUTDOWN: &str = "runner.shutdown";
pub const STAGE_INBOUND: &str = "runner.inbound";
pub const INBOUND_READ_TIMEOUT_SECS: u64 = 3;
pub const INBOUND_ACCEPT_RETRY_BACKOFF_MS: u64 = 25;
pub const HEARTBEAT_RPC_TIMEOUT_SECS: u64 = 2;
pub const MAX_INBOUND_CONNECTION_HANDLERS: usize = 32;
pub const MAX_CONSECUTIVE_HEARTBEAT_FAILURES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatDecision {
    Continue,
    Shutdown,
}

#[async_trait]
pub trait RunnerManagerClient: Send + Sync {
    async fn register(
        &self,
        bootstrap: &RunnerBootstrap,
        manager_auth_proof: String,
    ) -> Result<ControlResponse, ImagodError>;

    async fn mark_ready(
        &self,
        bootstrap: &RunnerBootstrap,
        manager_auth_proof: String,
    ) -> Result<ControlResponse, ImagodError>;

    async fn heartbeat(
        &self,
        bootstrap: &RunnerBootstrap,
        manager_auth_proof: String,
    ) -> Result<ControlResponse, ImagodError>;
}

#[derive(Debug, Clone, Default)]
pub struct DbusRunnerManagerClient;

#[async_trait]
impl RunnerManagerClient for DbusRunnerManagerClient {
    async fn register(
        &self,
        bootstrap: &RunnerBootstrap,
        manager_auth_proof: String,
    ) -> Result<ControlResponse, ImagodError> {
        DbusP2pTransport::call_control(
            &bootstrap.manager_control_endpoint,
            &ControlRequest::RegisterRunner {
                runner_id: bootstrap.runner_id.clone(),
                service_name: bootstrap.service_name.clone(),
                release_hash: bootstrap.release_hash.clone(),
                runner_endpoint: bootstrap.runner_endpoint.clone(),
                manager_auth_proof,
            },
        )
        .await
    }

    async fn mark_ready(
        &self,
        bootstrap: &RunnerBootstrap,
        manager_auth_proof: String,
    ) -> Result<ControlResponse, ImagodError> {
        DbusP2pTransport::call_control(
            &bootstrap.manager_control_endpoint,
            &ControlRequest::RunnerReady {
                runner_id: bootstrap.runner_id.clone(),
                manager_auth_proof,
            },
        )
        .await
    }

    async fn heartbeat(
        &self,
        bootstrap: &RunnerBootstrap,
        manager_auth_proof: String,
    ) -> Result<ControlResponse, ImagodError> {
        DbusP2pTransport::call_control(
            &bootstrap.manager_control_endpoint,
            &ControlRequest::Heartbeat {
                runner_id: bootstrap.runner_id.clone(),
                manager_auth_proof,
            },
        )
        .await
    }
}

pub async fn register<M>(bootstrap: &RunnerBootstrap, manager_client: &M) -> Result<(), ImagodError>
where
    M: RunnerManagerClient,
{
    let proof = compute_manager_auth_proof(&bootstrap.manager_auth_secret, &bootstrap.runner_id)?;
    let response = manager_client.register(bootstrap, proof).await?;
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

pub async fn mark_ready<M>(
    bootstrap: &RunnerBootstrap,
    manager_client: &M,
) -> Result<(), ImagodError>
where
    M: RunnerManagerClient,
{
    let proof = compute_manager_auth_proof(&bootstrap.manager_auth_secret, &bootstrap.runner_id)?;
    let response = manager_client.mark_ready(bootstrap, proof).await?;
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
pub async fn send_heartbeats(
    bootstrap: RunnerBootstrap,
    manager_client: impl RunnerManagerClient,
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
            manager_client.heartbeat(&bootstrap, proof),
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

pub fn evaluate_heartbeat_result(
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

pub fn apply_retryable_heartbeat_failure(consecutive_failures: &mut u32) -> HeartbeatDecision {
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
pub async fn run_inbound_server(
    listener: UnixListener,
    bootstrap: RunnerBootstrap,
    runtime_invoker: Arc<dyn RuntimeInvoker>,
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

        let (stream, _) = match accepted {
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
        let runtime_invoker = runtime_invoker.clone();
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut stream = stream;
            handle_inbound_connection(&mut stream, bootstrap, runtime_invoker, shutdown_tx).await;
        });
    }
}

pub fn should_retry_inbound_accept(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::TimedOut
    )
}

pub async fn handle_inbound_connection(
    stream: &mut UnixStream,
    bootstrap: RunnerBootstrap,
    runtime_invoker: Arc<dyn RuntimeInvoker>,
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

    let response =
        handle_inbound_request(&bootstrap, runtime_invoker.as_ref(), request, &shutdown_tx).await;
    let _ = DbusP2pTransport::write_message(stream, &response).await;
}

pub fn validate_shutdown_auth(
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
pub async fn handle_inbound_request(
    bootstrap: &RunnerBootstrap,
    runtime_invoker: &dyn RuntimeInvoker,
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
            payload_cbor,
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

            let invoke_request = RuntimeInvokeRequest {
                app_type: bootstrap.app_type,
                runner_id: bootstrap.runner_id.clone(),
                service_name: bootstrap.service_name.clone(),
                release_hash: bootstrap.release_hash.clone(),
                component_path: bootstrap.component_path.clone(),
                args: bootstrap.args.clone(),
                envs: bootstrap.envs.clone(),
                plugin_dependencies: bootstrap.plugin_dependencies.clone(),
                capabilities: bootstrap.capabilities.clone(),
                bindings: bootstrap.bindings.clone(),
                manager_control_endpoint: bootstrap.manager_control_endpoint.clone(),
                manager_auth_secret: bootstrap.manager_auth_secret.clone(),
                interface_id,
                function,
                payload_cbor,
            };

            match runtime_invoker.invoke_component(invoke_request).await {
                Ok(result_cbor) => RunnerInboundResponse::InvokeResult {
                    payload_cbor: result_cbor,
                },
                Err(err) => RunnerInboundResponse::Error(IpcErrorPayload::from_error(&err)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use imagod_ipc::{
        ControlRequest, InvocationTokenClaims, RunnerAppType, issue_invocation_token,
        now_unix_secs, random_secret_hex,
    };
    use tokio::sync::oneshot;

    struct EchoRuntimeInvoker;

    #[async_trait]
    impl RuntimeInvoker for EchoRuntimeInvoker {
        async fn invoke_component(
            &self,
            request: RuntimeInvokeRequest,
        ) -> Result<Vec<u8>, ImagodError> {
            Ok(request.payload_cbor)
        }
    }

    fn new_test_root(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        PathBuf::from(format!("/tmp/iss-runner-{prefix}-{ts}"))
    }

    fn new_test_bootstrap(root: &Path, runner_socket_name: &str) -> RunnerBootstrap {
        let runner_id = "runner-1".to_string();
        RunnerBootstrap {
            runner_id: runner_id.clone(),
            service_name: "svc-test".to_string(),
            release_hash: "release-test".to_string(),
            app_type: RunnerAppType::Cli,
            http_port: None,
            http_max_body_bytes: None,
            socket: None,
            component_path: root.join("component.wasm"),
            args: Vec::new(),
            envs: BTreeMap::new(),
            bindings: Vec::new(),
            plugin_dependencies: Vec::new(),
            capabilities: imagod_ipc::CapabilityPolicy::default(),
            manager_control_endpoint: root.join("manager-control.sock"),
            runner_endpoint: root.join(runner_socket_name),
            manager_auth_secret: random_secret_hex(),
            invocation_secret: random_secret_hex(),
            http_worker_count: 2,
            http_worker_queue_capacity: 4,
            epoch_tick_interval_ms: 50,
        }
    }

    fn prepare_socket_path_for_test(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("socket parent should be created");
        }
        if path.exists() {
            std::fs::remove_file(path).expect("stale socket should be removed");
        }
    }

    #[tokio::test]
    async fn shutdown_runner_rejects_invalid_manager_auth_proof() {
        let root = new_test_root("shutdown-auth");
        let bootstrap = new_test_bootstrap(&root, "runner.sock");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let runtime_invoker = EchoRuntimeInvoker;
        let response = handle_inbound_request(
            &bootstrap,
            &runtime_invoker,
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
        let runtime_invoker = EchoRuntimeInvoker;
        let response = handle_inbound_request(
            &bootstrap,
            &runtime_invoker,
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
    async fn invoke_returns_payload_cbor_on_valid_token() {
        let root = new_test_root("invoke-success");
        let bootstrap = new_test_bootstrap(&root, "runner.sock");
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let payload_cbor = vec![0x01, 0x02, 0x03];

        let token = issue_invocation_token(
            &bootstrap.invocation_secret,
            InvocationTokenClaims {
                source_service: "remote".to_string(),
                target_service: bootstrap.service_name.clone(),
                wit: "yieldspace:service/invoke".to_string(),
                exp: now_unix_secs() + 30,
                nonce: "nonce-invoke-test".to_string(),
            },
        )
        .expect("token should be issued");

        let runtime_invoker = EchoRuntimeInvoker;
        let response = handle_inbound_request(
            &bootstrap,
            &runtime_invoker,
            RunnerInboundRequest::Invoke {
                interface_id: "yieldspace:service/invoke".to_string(),
                function: "call".to_string(),
                payload_cbor: payload_cbor.clone(),
                token,
            },
            &shutdown_tx,
        )
        .await;

        match response {
            RunnerInboundResponse::InvokeResult {
                payload_cbor: actual,
            } => {
                assert_eq!(actual, payload_cbor);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn inbound_server_keeps_accepting_when_first_connection_stalls() {
        let root = new_test_root("inbound-hol");
        let bootstrap = new_test_bootstrap(&root, "runner.sock");
        prepare_socket_path_for_test(&bootstrap.runner_endpoint);
        let listener =
            UnixListener::bind(&bootstrap.runner_endpoint).expect("runner listener should bind");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut shutdown_observer = shutdown_tx.subscribe();
        let server_task = tokio::spawn(run_inbound_server(
            listener,
            bootstrap.clone(),
            Arc::new(EchoRuntimeInvoker),
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

        let manager_client = DbusRunnerManagerClient;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let heartbeat_task = tokio::spawn(send_heartbeats(
            bootstrap.clone(),
            manager_client,
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
