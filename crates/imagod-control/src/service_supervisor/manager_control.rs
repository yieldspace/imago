use std::{collections::BTreeMap, sync::Arc, time::Duration};

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

#[derive(Debug, Default)]
pub(super) struct DefaultManagerControlHandler;

impl DefaultManagerControlHandler {
    pub(super) async fn handle_control_request(
        &self,
        inner: &Arc<RwLock<BTreeMap<String, super::RunningService>>>,
        pending_ready: &Arc<Mutex<super::PendingReadyMap>>,
        request: ControlRequest,
    ) -> ControlResponse {
        handle_control_request_impl(inner, pending_ready, request).await
    }

    pub(super) async fn handle_control_connection(
        &self,
        stream: UnixStream,
        inner: Arc<RwLock<BTreeMap<String, super::RunningService>>>,
        pending_ready: Arc<Mutex<super::PendingReadyMap>>,
        read_timeout: Duration,
    ) {
        handle_control_connection_impl(stream, inner, pending_ready, read_timeout, self).await;
    }
}

pub(super) async fn handle_control_request_impl(
    inner: &Arc<RwLock<BTreeMap<String, super::RunningService>>>,
    pending_ready: &Arc<Mutex<super::PendingReadyMap>>,
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
    }
}

pub(super) async fn handle_control_connection_impl(
    mut stream: UnixStream,
    inner: Arc<RwLock<BTreeMap<String, super::RunningService>>>,
    pending_ready: Arc<Mutex<super::PendingReadyMap>>,
    read_timeout: Duration,
    handler: &DefaultManagerControlHandler,
) {
    let request = match time::timeout(
        read_timeout,
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
                    read_timeout.as_millis()
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
        .any(|binding| binding.target == target_service && binding.wit == wit)
}
