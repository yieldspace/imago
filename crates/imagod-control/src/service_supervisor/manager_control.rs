use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{
    ControlRequest, ControlResponse, IpcErrorPayload, dbus_p2p::DbusP2pTransport,
    issue_invocation_token, now_unix_secs, verify_manager_auth_proof,
};
use tokio::{
    net::UnixStream,
    sync::{Mutex, RwLock},
    time,
};

#[derive(Debug)]
pub(super) struct DefaultManagerControlHandler {
    remote_rpc: Mutex<super::remote_rpc::RemoteRpcManager>,
}

impl DefaultManagerControlHandler {
    pub(super) fn new(config_path: PathBuf) -> Self {
        Self {
            remote_rpc: Mutex::new(super::remote_rpc::RemoteRpcManager::new(config_path)),
        }
    }

    pub(super) async fn handle_control_request(
        &self,
        inner: &Arc<RwLock<BTreeMap<String, super::RunningService>>>,
        pending_ready: &Arc<Mutex<super::PendingReadyMap>>,
        request: ControlRequest,
    ) -> ControlResponse {
        handle_control_request_impl(inner, pending_ready, self, request).await
    }

    pub(super) async fn handle_control_connection(
        &self,
        stream: UnixStream,
        inner: Arc<RwLock<BTreeMap<String, super::RunningService>>>,
        pending_ready: Arc<Mutex<super::PendingReadyMap>>,
        manager_control_read_timeout: Duration,
    ) {
        handle_control_connection_impl(
            stream,
            inner,
            pending_ready,
            manager_control_read_timeout,
            self,
        )
        .await;
    }
}

pub(super) async fn handle_control_request_impl(
    inner: &Arc<RwLock<BTreeMap<String, super::RunningService>>>,
    pending_ready: &Arc<Mutex<super::PendingReadyMap>>,
    handler: &DefaultManagerControlHandler,
    request: ControlRequest,
) -> ControlResponse {
    match request {
        ControlRequest::RegisterRunner {
            runner_id,
            service_name,
            release_hash,
            runner_endpoint,
            manager_auth_proof,
        } => {
            let mut guard = inner.write().await;
            let Some((actual_service_name, service)) = guard
                .iter_mut()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "runner is not registered for startup");
            };

            if let Err(err) = validate_manager_auth(
                &service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }

            if actual_service_name != &service_name || service.release_hash != release_hash {
                return control_error(ErrorCode::BadRequest, "register_runner metadata mismatch");
            }

            if service.runner_endpoint != runner_endpoint {
                return control_error(ErrorCode::BadRequest, "register_runner endpoint mismatch");
            }
            service.last_heartbeat_at = now_unix_secs().to_string();
            ControlResponse::Ack
        }
        ControlRequest::RunnerReady {
            runner_id,
            manager_auth_proof,
        } => {
            {
                let mut guard = inner.write().await;
                let Some((_, service)) = guard
                    .iter_mut()
                    .find(|(_, service)| service.runner_id == runner_id)
                else {
                    return control_error(ErrorCode::NotFound, "runner is not registered");
                };

                if let Err(err) = validate_manager_auth(
                    &service.manager_auth_secret,
                    &runner_id,
                    &manager_auth_proof,
                ) {
                    return ControlResponse::Error(IpcErrorPayload::from_error(&err));
                }

                service.is_ready = true;
                service.last_heartbeat_at = now_unix_secs().to_string();
            }

            if let Some(sender) = pending_ready.lock().await.remove(&runner_id) {
                let _ = sender.send(Ok(()));
            }
            ControlResponse::Ack
        }
        ControlRequest::Heartbeat {
            runner_id,
            manager_auth_proof,
        } => {
            let mut guard = inner.write().await;
            let Some((_, service)) = guard
                .iter_mut()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "runner is not registered");
            };

            if let Err(err) = validate_manager_auth(
                &service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }

            service.last_heartbeat_at = now_unix_secs().to_string();
            ControlResponse::Ack
        }
        ControlRequest::ResolveInvocationTarget {
            runner_id,
            manager_auth_proof,
            target_service,
            wit,
        } => {
            let guard = inner.read().await;

            let Some((source_service_name, source_service)) = guard
                .iter()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "source runner is not registered");
            };

            if let Err(err) = validate_manager_auth(
                &source_service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }

            if !is_binding_allowed(&source_service.bindings, &target_service, &wit) {
                return control_error(
                    ErrorCode::Unauthorized,
                    "binding does not allow target service/interface",
                );
            }

            let Some(target_runner) = guard.get(&target_service) else {
                return control_error(ErrorCode::NotFound, "target service is not running");
            };
            if !target_runner.is_ready {
                return control_error(ErrorCode::NotFound, "target service is not running");
            }

            let claims = imagod_ipc::InvocationTokenClaims {
                source_service: source_service_name.clone(),
                target_service: target_service.clone(),
                wit: wit.clone(),
                exp: now_unix_secs() + super::INVOCATION_TOKEN_TTL_SECS,
                nonce: uuid::Uuid::new_v4().to_string(),
            };
            let token = match issue_invocation_token(&target_runner.invocation_secret, claims) {
                Ok(token) => token,
                Err(err) => return ControlResponse::Error(IpcErrorPayload::from_error(&err)),
            };

            ControlResponse::ResolvedInvocationTarget {
                endpoint: target_runner.runner_endpoint.clone(),
                token,
            }
        }
        ControlRequest::RpcConnectRemote {
            runner_id,
            manager_auth_proof,
            authority,
        } => {
            let guard = inner.read().await;
            let Some((_, source_service)) = guard
                .iter()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "source runner is not registered");
            };

            if let Err(err) = validate_manager_auth(
                &source_service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }
            drop(guard);

            let config_path = {
                let remote_rpc = handler.remote_rpc.lock().await;
                remote_rpc.config_path().to_path_buf()
            };
            let authority = match super::remote_rpc::RemoteRpcManager::probe_remote_authority(
                &config_path,
                &authority,
            )
            .await
            {
                Ok(authority) => authority,
                Err(err) => return ControlResponse::Error(IpcErrorPayload::from_error(&err)),
            };

            let mut remote_rpc = handler.remote_rpc.lock().await;
            let connection_id = remote_rpc.insert_connection(&runner_id, authority);
            ControlResponse::RpcRemoteConnected { connection_id }
        }
        ControlRequest::RpcInvokeRemote {
            runner_id,
            manager_auth_proof,
            connection_id,
            target_service,
            interface_id,
            function,
            args_cbor,
        } => {
            let guard = inner.read().await;
            let Some((_, source_service)) = guard
                .iter()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "source runner is not registered");
            };

            if let Err(err) = validate_manager_auth(
                &source_service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }
            if !is_binding_allowed(&source_service.bindings, &target_service, &interface_id) {
                return control_error(
                    ErrorCode::Unauthorized,
                    "binding does not allow target service/interface",
                );
            }
            drop(guard);

            let (remote_authority, config_path) = {
                let remote_rpc = handler.remote_rpc.lock().await;
                (
                    remote_rpc.connection_for(&runner_id, &connection_id),
                    remote_rpc.config_path().to_path_buf(),
                )
            };
            let Some(remote_authority) = remote_authority else {
                return control_error(
                    ErrorCode::NotFound,
                    format!("rpc connection '{connection_id}' is not available"),
                );
            };

            match super::remote_rpc::RemoteRpcManager::invoke_with_authority(
                &config_path,
                &remote_authority,
                &target_service,
                &interface_id,
                &function,
                &args_cbor,
            )
            .await
            {
                Ok(result_cbor) => ControlResponse::RpcRemoteInvokeResult { result_cbor },
                Err(err) => ControlResponse::Error(IpcErrorPayload::from_error(&err)),
            }
        }
        ControlRequest::RpcDisconnectRemote {
            runner_id,
            manager_auth_proof,
            connection_id,
        } => {
            let guard = inner.read().await;
            let Some((_, source_service)) = guard
                .iter()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "source runner is not registered");
            };

            if let Err(err) = validate_manager_auth(
                &source_service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }
            drop(guard);

            let mut remote_rpc = handler.remote_rpc.lock().await;
            if remote_rpc.disconnect(&runner_id, &connection_id) {
                ControlResponse::Ack
            } else {
                control_error(
                    ErrorCode::NotFound,
                    format!("rpc connection '{connection_id}' is not available"),
                )
            }
        }
    }
}

pub(super) async fn handle_control_connection_impl(
    mut stream: UnixStream,
    inner: Arc<RwLock<BTreeMap<String, super::RunningService>>>,
    pending_ready: Arc<Mutex<super::PendingReadyMap>>,
    manager_control_read_timeout: Duration,
    handler: &DefaultManagerControlHandler,
) {
    let request = match time::timeout(
        manager_control_read_timeout,
        DbusP2pTransport::read_message::<ControlRequest>(&mut stream),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(err)) => {
            let _ = DbusP2pTransport::write_message(
                &mut stream,
                &ControlResponse::Error(IpcErrorPayload::from_error(&err)),
            )
            .await;
            return;
        }
        Err(_) => {
            let timeout_error = IpcErrorPayload {
                code: ErrorCode::OperationTimeout,
                stage: super::STAGE_CONTROL.to_string(),
                message: format!(
                    "manager control request read timed out after {} ms",
                    manager_control_read_timeout.as_millis()
                ),
            };
            if let Err(err) =
                DbusP2pTransport::write_message(&mut stream, &ControlResponse::Error(timeout_error))
                    .await
            {
                eprintln!("manager control timeout response write failed: {err}");
            }
            return;
        }
    };

    let response = handler
        .handle_control_request(&inner, &pending_ready, request)
        .await;
    let _ = DbusP2pTransport::write_message(&mut stream, &response).await;
}

/// Validates manager proof generated from shared secret and runner id.
pub(super) fn validate_manager_auth(
    secret: &str,
    runner_id: &str,
    proof: &str,
) -> Result<(), ImagodError> {
    match verify_manager_auth_proof(secret, runner_id, proof) {
        Ok(()) => Ok(()),
        Err(err) if err.code == ErrorCode::Unauthorized => Err(ImagodError::new(
            ErrorCode::Unauthorized,
            super::STAGE_CONTROL,
            "manager auth proof mismatch",
        )),
        Err(err) => Err(ImagodError::new(
            err.code,
            super::STAGE_CONTROL,
            format!("manager auth proof verification failed: {}", err.message),
        )),
    }
}

pub(super) fn control_error(code: ErrorCode, message: impl Into<String>) -> ControlResponse {
    ControlResponse::Error(IpcErrorPayload {
        code,
        stage: super::STAGE_CONTROL.to_string(),
        message: message.into(),
    })
}

/// Returns whether a binding list allows the target service/interface pair.
pub(super) fn is_binding_allowed(
    bindings: &[imagod_ipc::ServiceBinding],
    target_service: &str,
    wit: &str,
) -> bool {
    bindings
        .iter()
        .any(|binding| binding.name == target_service && binding.wit == wit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_ipc::{compute_manager_auth_proof, random_secret_hex};
    use std::{process::Stdio, sync::Arc};
    use tokio::{
        process::{Child, Command},
        sync::broadcast,
    };

    fn new_running_service(
        child: Child,
        runner_id: &str,
        manager_auth_secret: String,
        bindings: Vec<imagod_ipc::ServiceBinding>,
    ) -> super::super::RunningService {
        let (log_sender, _) = broadcast::channel(16);
        super::super::RunningService {
            release_hash: "release-test".to_string(),
            started_at: now_unix_secs().to_string(),
            status: super::super::RunningStatus::Running,
            is_ready: true,
            runner_id: runner_id.to_string(),
            runner_endpoint: PathBuf::from(format!("/tmp/{runner_id}.sock")),
            manager_auth_secret,
            invocation_secret: random_secret_hex(),
            bindings,
            child,
            _stdout_log: Arc::new(Mutex::new(super::super::log_buffer::BoundedLogBuffer::new(
                64,
            ))),
            _stderr_log: Arc::new(Mutex::new(super::super::log_buffer::BoundedLogBuffer::new(
                64,
            ))),
            composite_log: Arc::new(Mutex::new(super::super::log_buffer::BoundedLogBuffer::new(
                128,
            ))),
            composite_events: Arc::new(Mutex::new(
                super::super::log_buffer::BoundedLogEventBuffer::new(128),
            )),
            log_sender,
            last_heartbeat_at: now_unix_secs().to_string(),
        }
    }

    async fn stop_running_service_best_effort(
        inner: &Arc<RwLock<BTreeMap<String, super::super::RunningService>>>,
        service_name: &str,
    ) {
        let service = {
            let mut guard = inner.write().await;
            guard.remove(service_name)
        };
        if let Some(mut service) = service {
            let _ = service.child.start_kill();
            let _ = service.child.wait().await;
        }
    }

    #[tokio::test]
    async fn rpc_connect_remote_rejects_mismatched_manager_auth_proof() {
        let inner: Arc<RwLock<BTreeMap<String, super::super::RunningService>>> =
            Arc::new(RwLock::new(BTreeMap::new()));
        let pending_ready: Arc<Mutex<super::super::PendingReadyMap>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let service_name = "svc-rpc-connect";
        let runner_id = "runner-rpc-connect";
        let manager_auth_secret = random_secret_hex();
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("runner child should spawn");
        let service = new_running_service(child, runner_id, manager_auth_secret, Vec::new());
        {
            let mut guard = inner.write().await;
            guard.insert(service_name.to_string(), service);
        }

        let handler =
            DefaultManagerControlHandler::new(PathBuf::from("/tmp/imagod-control-test.toml"));
        let response = handle_control_request_impl(
            &inner,
            &pending_ready,
            &handler,
            ControlRequest::RpcConnectRemote {
                runner_id: runner_id.to_string(),
                manager_auth_proof: "invalid-proof".to_string(),
                authority: "rpc://example.com".to_string(),
            },
        )
        .await;

        match response {
            ControlResponse::Error(err) => {
                assert_eq!(err.code, ErrorCode::Unauthorized);
                assert_eq!(err.message, "manager auth proof mismatch");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        stop_running_service_best_effort(&inner, service_name).await;
    }

    #[tokio::test]
    async fn rpc_invoke_remote_rejects_binding_mismatch() {
        let inner: Arc<RwLock<BTreeMap<String, super::super::RunningService>>> =
            Arc::new(RwLock::new(BTreeMap::new()));
        let pending_ready: Arc<Mutex<super::super::PendingReadyMap>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let service_name = "svc-rpc-invoke";
        let runner_id = "runner-rpc-invoke";
        let manager_auth_secret = random_secret_hex();
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("runner child should spawn");
        let service = new_running_service(
            child,
            runner_id,
            manager_auth_secret.clone(),
            vec![imagod_ipc::ServiceBinding {
                name: "svc-allowed".to_string(),
                wit: "pkg:iface/allowed".to_string(),
            }],
        );
        {
            let mut guard = inner.write().await;
            guard.insert(service_name.to_string(), service);
        }

        let manager_auth_proof = compute_manager_auth_proof(&manager_auth_secret, runner_id)
            .expect("manager auth proof should be generated");
        let handler =
            DefaultManagerControlHandler::new(PathBuf::from("/tmp/imagod-control-test.toml"));
        let response = handle_control_request_impl(
            &inner,
            &pending_ready,
            &handler,
            ControlRequest::RpcInvokeRemote {
                runner_id: runner_id.to_string(),
                manager_auth_proof,
                connection_id: "connection-1".to_string(),
                target_service: "svc-target".to_string(),
                interface_id: "pkg:iface/invoke".to_string(),
                function: "call".to_string(),
                args_cbor: vec![0x01],
            },
        )
        .await;

        match response {
            ControlResponse::Error(err) => {
                assert_eq!(err.code, ErrorCode::Unauthorized);
                assert_eq!(
                    err.message,
                    "binding does not allow target service/interface"
                );
            }
            other => panic!("unexpected response: {other:?}"),
        }

        stop_running_service_best_effort(&inner, service_name).await;
    }
}
