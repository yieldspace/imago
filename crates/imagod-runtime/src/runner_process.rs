//! Runner process bootstrap and inbound control loop.

use std::path::Path;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{
    ControlRequest, ControlResponse, IpcErrorPayload, RunnerBootstrap, RunnerInboundRequest,
    RunnerInboundResponse, compute_manager_auth_proof, dbus_p2p::DbusP2pTransport,
    verify_invocation_token,
};
use tokio::{
    io::AsyncReadExt,
    net::{UnixListener, UnixStream},
    sync::watch,
    time::{self, Duration},
};

use crate::runtime_wasmtime::WasmRuntime;

const STAGE_RUNNER: &str = "runner.process";
const STAGE_SHUTDOWN: &str = "runner.shutdown";
const STAGE_INBOUND: &str = "runner.inbound";
const INBOUND_READ_TIMEOUT_SECS: u64 = 3;

/// Starts runner mode by reading `RunnerBootstrap` from stdin and executing the component.
///
/// The function registers the runner with manager, serves inbound IPC requests,
/// emits heartbeat signals, and exits when the component finishes or shutdown is requested.
pub async fn run_runner_from_stdin() -> Result<(), ImagodError> {
    let mut stdin = tokio::io::stdin();
    let mut bootstrap_bytes = Vec::new();
    stdin.read_to_end(&mut bootstrap_bytes).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNNER,
            format!("failed to read runner bootstrap from stdin: {e}"),
        )
    })?;
    let bootstrap =
        imago_protocol::from_cbor::<RunnerBootstrap>(&bootstrap_bytes).map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_RUNNER,
                format!("failed to decode runner bootstrap: {e}"),
            )
        })?;

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

    let runtime = WasmRuntime::new()?;
    runtime.validate_component(&bootstrap.component_path)?;

    register_runner(&bootstrap).await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let inbound_task = tokio::spawn(run_inbound_server(
        listener,
        bootstrap.clone(),
        shutdown_tx.clone(),
        shutdown_rx.clone(),
    ));

    mark_ready(&bootstrap).await?;

    let heartbeat_task = tokio::spawn(send_heartbeats(bootstrap.clone(), shutdown_rx.clone()));

    let run_result = runtime
        .run_cli_component_async(
            &bootstrap.component_path,
            &bootstrap.args,
            &bootstrap.envs,
            shutdown_rx,
            bootstrap.epoch_tick_interval_ms,
        )
        .await;

    let _ = shutdown_tx.send(true);
    let _ = heartbeat_task.await;
    let _ = inbound_task.await;
    let _ = std::fs::remove_file(&bootstrap.runner_endpoint);

    run_result
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
async fn send_heartbeats(bootstrap: RunnerBootstrap, mut shutdown: watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            break;
        }
        let proof = match compute_manager_auth_proof(
            &bootstrap.manager_auth_secret,
            &bootstrap.runner_id,
        ) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("runner heartbeat auth error: {err}");
                break;
            }
        };
        let _ = DbusP2pTransport::call_control(
            &bootstrap.manager_control_endpoint,
            &ControlRequest::Heartbeat {
                runner_id: bootstrap.runner_id.clone(),
                manager_auth_proof: proof,
            },
        )
        .await;

        tokio::select! {
            _ = time::sleep(Duration::from_secs(1)) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
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
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        };

        let (mut stream, _) = match accepted {
            Ok(v) => v,
            Err(err) => {
                eprintln!("runner inbound accept error: {err}");
                break;
            }
        };

        let bootstrap = bootstrap.clone();
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            handle_inbound_connection(&mut stream, bootstrap, shutdown_tx).await;
        });
    }
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
    let expected = compute_manager_auth_proof(manager_auth_secret, runner_id)?;
    if manager_auth_proof == expected {
        Ok(())
    } else {
        Err(ImagodError::new(
            ErrorCode::Unauthorized,
            STAGE_SHUTDOWN,
            "manager auth proof mismatch",
        ))
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
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use imagod_ipc::{RunnerInboundResponse, random_secret_hex};

    use super::*;

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
}
