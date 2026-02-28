use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use imago_protocol::messages::{
    BindingsCertUploadRequest, BindingsCertUploadResponse, RpcInvokeRequest, RpcInvokeResponse,
};
use imago_protocol::{
    ArtifactPushRequest, CommandCancelRequest, CommandEventType, CommandPayload,
    CommandStartRequest, CommandStartResponse, CommandState, CommandType, DeployPrepareRequest,
    MessageType, PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSION_RANGE, ServiceListRequest,
    ServiceListResponse, StateRequest, Validate,
};
use imagod_common::ImagodError;
use imagod_config::upsert_tls_known_client_key;
use imagod_control::{OperationManager, SpawnTransition};
use semver::{Version, VersionReq};
use serde::Serialize;
use web_transport_quinn::SendStream;

use super::{
    Envelope, ProtocolHandler,
    envelope_io::{bad_request, event_envelope, payload_take, response_envelope, write_envelope},
    logs_forwarder::LogsForwarder,
    session_loop::ProtocolSession,
    upsert_dynamic_client_public_key,
};

impl ProtocolHandler {
    /// Dispatches non-command-start requests to the corresponding handler.
    pub(crate) async fn handle_single(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        match request.message_type {
            MessageType::HelloNegotiate => self.handle_hello(request),
            MessageType::DeployPrepare => self.handle_prepare(request).await,
            MessageType::ArtifactPush => self.handle_push(request).await,
            MessageType::ArtifactCommit => self.handle_commit(request).await,
            MessageType::StateRequest => self.handle_state_request(request).await,
            MessageType::ServicesList => self.handle_services_list(request).await,
            MessageType::CommandCancel => self.handle_command_cancel(request).await,
            MessageType::RpcInvoke => self.handle_rpc_invoke(request).await,
            MessageType::BindingsCertUpload => self.handle_bindings_cert_upload(request),
            _ => Err(ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "dispatch",
                "unsupported message type",
            )),
        }
    }

    fn handle_hello(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: imago_protocol::HelloNegotiateRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("hello.negotiate", e.to_string()))?;

        let compatibility_announcement =
            protocol_compatibility_announcement(&payload.client_version);
        let accepted = compatibility_announcement.is_none();
        let mut limits = BTreeMap::new();
        limits.insert(
            "chunk_size".to_string(),
            self.config.runtime.chunk_size.to_string(),
        );
        limits.insert(
            "max_inflight_chunks".to_string(),
            self.config.runtime.max_inflight_chunks.to_string(),
        );
        limits.insert(
            "max_artifact_size_bytes".to_string(),
            self.config.runtime.max_artifact_size_bytes.to_string(),
        );
        limits.insert(
            "upload_session_ttl".to_string(),
            format!("{}s", self.config.runtime.upload_session_ttl_secs),
        );
        limits.insert(
            "deploy_stream_timeout_secs".to_string(),
            self.config.runtime.deploy_stream_timeout_secs.to_string(),
        );

        response_envelope(
            MessageType::HelloNegotiate,
            request_id,
            correlation_id,
            &imago_protocol::HelloNegotiateResponse {
                accepted,
                server_version: self.config.server_version.clone(),
                server_protocol_version: PROTOCOL_VERSION.to_string(),
                supported_protocol_version_range: SUPPORTED_PROTOCOL_VERSION_RANGE.to_string(),
                compatibility_announcement,
                features: vec![
                    "hello.negotiate".to_string(),
                    "deploy.prepare".to_string(),
                    "artifact.push".to_string(),
                    "artifact.commit".to_string(),
                    "command.start".to_string(),
                    "command.event".to_string(),
                    "state.request".to_string(),
                    "services.list".to_string(),
                    "command.cancel".to_string(),
                    "logs.request".to_string(),
                    "logs.chunk".to_string(),
                    "logs.chunk.timestamp".to_string(),
                    "logs.end".to_string(),
                    "rpc.invoke".to_string(),
                    "bindings.cert.upload".to_string(),
                ],
                limits,
            },
        )
    }

    async fn handle_prepare(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: DeployPrepareRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("deploy.prepare", e.to_string()))?;

        let response = self.artifacts.prepare(payload).await?;
        response_envelope(
            MessageType::DeployPrepare,
            request_id,
            correlation_id,
            &response,
        )
    }

    async fn handle_push(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: ArtifactPushRequest = payload_take(&mut request)?;
        self.handle_push_typed(request_id, correlation_id, payload)
            .await
    }

    pub(crate) async fn handle_push_typed(
        &self,
        request_id: uuid::Uuid,
        correlation_id: uuid::Uuid,
        payload: ArtifactPushRequest,
    ) -> Result<Envelope, ImagodError> {
        validate_push_payload(&payload)?;

        let response = self.artifacts.push(payload).await?;
        response_envelope(
            MessageType::ArtifactPush,
            request_id,
            correlation_id,
            &response,
        )
    }

    async fn handle_commit(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: imago_protocol::ArtifactCommitRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("artifact.commit", e.to_string()))?;

        let response = self.artifacts.commit(payload).await?;
        response_envelope(
            MessageType::ArtifactCommit,
            request_id,
            correlation_id,
            &response,
        )
    }

    async fn handle_state_request(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: StateRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("state.request", e.to_string()))?;

        let response = self
            .operations
            .snapshot_running(&payload.request_id)
            .await?;
        response_envelope(
            MessageType::StateResponse,
            request_id,
            correlation_id,
            &response,
        )
    }

    async fn handle_services_list(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: ServiceListRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("services.list", e.to_string()))?;

        let response = ServiceListResponse {
            services: self
                .orchestrator
                .list_service_states(payload.names.as_deref())
                .await?,
        };
        response
            .validate()
            .map_err(|e| bad_request("services.list", e.to_string()))?;
        response_envelope(
            MessageType::ServicesList,
            request_id,
            correlation_id,
            &response,
        )
    }

    async fn handle_command_cancel(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: CommandCancelRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("command.cancel", e.to_string()))?;

        let response = self.operations.request_cancel(&payload.request_id).await?;
        response_envelope(
            MessageType::CommandCancel,
            request_id,
            correlation_id,
            &response,
        )
    }

    async fn handle_rpc_invoke(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: RpcInvokeRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("rpc.invoke", e.to_string()))?;

        let RpcInvokeRequest {
            interface_id,
            function,
            args_cbor,
            target_service,
        } = payload;
        let response = match self
            .orchestrator
            .invoke(&target_service.name, &interface_id, &function, args_cbor)
            .await
        {
            Ok(result_cbor) => RpcInvokeResponse::from_result(result_cbor),
            Err(err) => RpcInvokeResponse::from_error(err.code, err.stage, err.message),
        };

        response_envelope(
            MessageType::RpcInvoke,
            request_id,
            correlation_id,
            &response,
        )
    }

    fn handle_bindings_cert_upload(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: BindingsCertUploadRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("bindings.cert.upload", e.to_string()))?;

        upsert_tls_known_client_key(
            self.config_path.as_path(),
            &payload.authority,
            &payload.public_key_hex,
        )?;
        let updated = upsert_dynamic_client_public_key(&payload.public_key_hex)?;
        let detail = if updated {
            format!("registered client key for authority {}", payload.authority)
        } else {
            format!(
                "client key already registered for authority {}",
                payload.authority
            )
        };

        response_envelope(
            MessageType::BindingsCertUpload,
            request_id,
            correlation_id,
            &BindingsCertUploadResponse { updated, detail },
        )
    }

    /// Handles `logs.request`, returns stream ACK and starts datagram forwarding.
    pub(crate) async fn handle_logs_request<S>(
        &self,
        session: Arc<S>,
        mut request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError>
    where
        S: ProtocolSession + 'static,
    {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: imago_protocol::LogRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("logs.request", e.to_string()))?;
        let imago_protocol::LogRequest {
            name,
            tail_lines,
            follow,
            with_timestamp,
        } = payload;

        let loggable_names = if name.is_none() {
            self.orchestrator.loggable_service_names().await
        } else {
            Vec::new()
        };
        let service_names = resolve_logs_request_service_names(name, loggable_names)?;

        let mut subscriptions = Vec::with_capacity(service_names.len());
        for service_name in &service_names {
            let subscription = self
                .orchestrator
                .open_logs(service_name, tail_lines, follow)
                .await?;
            subscriptions.push(subscription);
        }

        #[derive(Serialize)]
        struct LogsRequestAck {
            accepted: bool,
            names: Vec<String>,
            follow: bool,
        }

        let ack = response_envelope(
            MessageType::LogsRequest,
            request_id,
            correlation_id,
            &LogsRequestAck {
                accepted: true,
                names: service_names,
                follow,
            },
        )?;
        write_envelope(send, &ack, self.frame_codec.as_ref()).await?;

        let logs_forwarder = self.logs_forwarder.clone();
        tokio::spawn(async move {
            logs_forwarder
                .forward(
                    session,
                    request_id,
                    correlation_id,
                    subscriptions,
                    with_timestamp,
                )
                .await;
        });

        Ok(())
    }

    /// Handles `command.start` and emits accepted/progress/terminal events.
    pub(crate) async fn handle_command_start(
        &self,
        mut request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: CommandStartRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("command.start", e.to_string()))?;

        ensure_command_start_request_id_match(request_id, payload.request_id)?;
        ensure_command_start_allowed(&self.shutdown_requested)?;
        let operation_id = request_id;
        let deploy_id_for_cleanup = if payload.command_type == CommandType::Deploy {
            match &payload.payload {
                CommandPayload::Deploy(deploy_payload) => Some(deploy_payload.deploy_id.clone()),
                _ => None,
            }
        } else {
            None
        };

        self.operations
            .start(operation_id, payload.command_type)
            .await?;

        let accepted = response_envelope(
            MessageType::CommandStart,
            request_id,
            correlation_id,
            &CommandStartResponse { accepted: true },
        )?;
        write_envelope(send, &accepted, self.frame_codec.as_ref()).await?;

        let accepted_event = event_envelope(
            self.clock.as_ref(),
            operation_id,
            correlation_id,
            CommandEventType::Accepted,
            payload.command_type,
            None,
            None,
        )?;
        write_envelope(send, &accepted_event, self.frame_codec.as_ref()).await?;

        self.operations
            .set_state(&operation_id, CommandState::Running, "starting")
            .await?;
        let running_event = event_envelope(
            self.clock.as_ref(),
            operation_id,
            correlation_id,
            CommandEventType::Progress,
            payload.command_type,
            Some("starting".to_string()),
            None,
        )?;
        write_envelope(send, &running_event, self.frame_codec.as_ref()).await?;

        let spawn_transition = self
            .operations
            .mark_spawned_if_not_canceled(&operation_id, "spawned")
            .await?;
        if spawn_transition == SpawnTransition::Canceled {
            let canceled = event_envelope(
                self.clock.as_ref(),
                operation_id,
                correlation_id,
                CommandEventType::Canceled,
                payload.command_type,
                Some("canceled".to_string()),
                None,
            )?;
            let canceled_write = write_envelope(send, &canceled, self.frame_codec.as_ref()).await;
            let finalize_result = finalize_operation_after_terminal_event(
                &self.operations,
                &operation_id,
                CommandState::Canceled,
                "canceled",
                canceled_write,
            )
            .await;
            if let Some(deploy_id) = deploy_id_for_cleanup.as_deref()
                && let Err(err) = self.artifacts.purge_deploy_session(deploy_id).await
            {
                eprintln!(
                    "artifact session purge failed deploy_id={} code={:?} stage={} message={}",
                    deploy_id, err.code, err.stage, err.message
                );
            }
            finalize_result?;
            return Ok(());
        }

        let command_result = match (&payload.command_type, &payload.payload) {
            (CommandType::Deploy, CommandPayload::Deploy(deploy_payload)) => self
                .orchestrator
                .deploy(deploy_payload)
                .await
                .map(|summary| {
                    (
                        format!("release:{}:{}", summary.service_name, summary.release_hash),
                        "spawned".to_string(),
                    )
                }),
            (CommandType::Run, CommandPayload::Run(run_payload)) => {
                self.orchestrator.run(run_payload).await.map(|summary| {
                    (
                        format!("running:{}:{}", summary.service_name, summary.release_hash),
                        "spawned".to_string(),
                    )
                })
            }
            (CommandType::Stop, CommandPayload::Stop(stop_payload)) => {
                self.orchestrator.stop(stop_payload).await.map(|summary| {
                    (
                        format!("stopped:{}", summary.service_name),
                        "completed".to_string(),
                    )
                })
            }
            _ => Err(ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "command.start",
                "payload does not match command_type",
            )),
        };

        match command_result {
            Ok((progress_stage, success_stage)) => {
                let success_stage_for_event = success_stage.clone();
                let progress = event_envelope(
                    self.clock.as_ref(),
                    operation_id,
                    correlation_id,
                    CommandEventType::Progress,
                    payload.command_type,
                    Some(progress_stage),
                    None,
                )?;
                write_envelope(send, &progress, self.frame_codec.as_ref()).await?;

                let succeeded = event_envelope(
                    self.clock.as_ref(),
                    operation_id,
                    correlation_id,
                    CommandEventType::Succeeded,
                    payload.command_type,
                    Some(success_stage_for_event),
                    None,
                )?;
                let succeeded_write =
                    write_envelope(send, &succeeded, self.frame_codec.as_ref()).await;
                let finalize_result = finalize_operation_after_terminal_event(
                    &self.operations,
                    &operation_id,
                    CommandState::Succeeded,
                    success_stage,
                    succeeded_write,
                )
                .await;
                if let Some(deploy_id) = deploy_id_for_cleanup.as_deref()
                    && let Err(err) = self.artifacts.purge_deploy_session(deploy_id).await
                {
                    eprintln!(
                        "artifact session purge failed deploy_id={} code={:?} stage={} message={}",
                        deploy_id, err.code, err.stage, err.message
                    );
                }
                finalize_result?;
            }
            Err(err) => {
                let failed = event_envelope(
                    self.clock.as_ref(),
                    operation_id,
                    correlation_id,
                    CommandEventType::Failed,
                    payload.command_type,
                    Some("failed".to_string()),
                    Some(err.to_structured()),
                )?;
                let failed_write = write_envelope(send, &failed, self.frame_codec.as_ref()).await;
                let finalize_result = finalize_operation_after_terminal_event(
                    &self.operations,
                    &operation_id,
                    CommandState::Failed,
                    "failed",
                    failed_write,
                )
                .await;
                if let Some(deploy_id) = deploy_id_for_cleanup.as_deref()
                    && let Err(err) = self.artifacts.purge_deploy_session(deploy_id).await
                {
                    eprintln!(
                        "artifact session purge failed deploy_id={} code={:?} stage={} message={}",
                        deploy_id, err.code, err.stage, err.message
                    );
                }
                finalize_result?;
            }
        }

        Ok(())
    }
}

pub(crate) fn protocol_compatibility_announcement(client_protocol_version: &str) -> Option<String> {
    let supported_range = match VersionReq::parse(SUPPORTED_PROTOCOL_VERSION_RANGE) {
        Ok(parsed) => parsed,
        Err(err) => {
            return Some(format!(
                "imagod protocol compatibility is misconfigured: supported range '{}' is invalid ({err})",
                SUPPORTED_PROTOCOL_VERSION_RANGE
            ));
        }
    };

    let client_version = match Version::parse(client_protocol_version) {
        Ok(parsed) => parsed,
        Err(_) => {
            return Some(format!(
                "client protocol version '{client_protocol_version}' is invalid; supported range is '{}' (server protocol '{}')",
                SUPPORTED_PROTOCOL_VERSION_RANGE, PROTOCOL_VERSION
            ));
        }
    };

    if supported_range.matches(&client_version) {
        return None;
    }

    Some(format!(
        "client protocol version '{client_protocol_version}' is not supported; imagod supports '{}' (server protocol '{}')",
        SUPPORTED_PROTOCOL_VERSION_RANGE, PROTOCOL_VERSION
    ))
}

pub(crate) fn ensure_command_start_request_id_match(
    envelope_request_id: uuid::Uuid,
    payload_request_id: uuid::Uuid,
) -> Result<(), ImagodError> {
    if envelope_request_id == payload_request_id {
        return Ok(());
    }

    Err(bad_request(
        "command.start",
        "envelope request_id and payload request_id must match",
    ))
}

pub(crate) fn ensure_command_start_allowed(
    shutdown_requested: &AtomicBool,
) -> Result<(), ImagodError> {
    if !shutdown_requested.load(Ordering::SeqCst) {
        return Ok(());
    }

    Err(ImagodError::new(
        imago_protocol::ErrorCode::Busy,
        "command.start",
        "server is shutting down",
    ))
}

pub(crate) fn validate_push_payload(payload: &ArtifactPushRequest) -> Result<(), ImagodError> {
    payload
        .validate()
        .map_err(|e| bad_request("artifact.push", e.to_string()))
}

fn resolve_logs_request_service_names(
    requested_name: Option<String>,
    loggable_names: Vec<String>,
) -> Result<Vec<String>, ImagodError> {
    match requested_name {
        Some(name) => Ok(vec![name]),
        None if !loggable_names.is_empty() => Ok(loggable_names),
        None => Err(ImagodError::new(
            imago_protocol::ErrorCode::NotFound,
            "logs.request",
            "no loggable services are available",
        )),
    }
}

/// Finalizes operation bookkeeping after writing terminal event.
pub(crate) async fn finalize_operation_after_terminal_event(
    operations: &OperationManager,
    request_id: &uuid::Uuid,
    terminal_state: CommandState,
    stage: impl Into<String>,
    terminal_write_result: Result<(), ImagodError>,
) -> Result<(), ImagodError> {
    operations
        .finish(request_id, terminal_state, stage.into())
        .await;
    operations.remove(request_id).await;
    terminal_write_result
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    use std::sync::atomic::AtomicBool;

    use imago_protocol::{ArtifactPushChunkHeader, ArtifactPushRequest, CommandState, CommandType};
    use imagod_control::OperationManager;
    use uuid::Uuid;

    use super::{
        ensure_command_start_allowed, ensure_command_start_request_id_match,
        finalize_operation_after_terminal_event, protocol_compatibility_announcement,
        resolve_logs_request_service_names, validate_push_payload,
    };
    use imago_protocol::ErrorCode;

    #[test]
    fn given_name_is_none__when_loggable_names_exist__then_resolve_logs_uses_all_names() {
        let names = resolve_logs_request_service_names(
            None,
            vec!["svc-running".to_string(), "svc-retained".to_string()],
        )
        .expect("name=None should use loggable names");
        assert_eq!(names, vec!["svc-running", "svc-retained"]);
    }

    #[test]
    fn given_name_is_none__when_no_loggable_service_exists__then_not_found_is_returned() {
        let err = resolve_logs_request_service_names(None, Vec::new())
            .expect_err("empty loggable names should be rejected");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(err.message, "no loggable services are available");
    }

    #[test]
    fn given_name_is_specified__when_resolve_logs_request_service_names__then_requested_name_is_used()
     {
        let names = resolve_logs_request_service_names(
            Some("svc-explicit".to_string()),
            vec!["svc-running".to_string()],
        )
        .expect("name=Some should be accepted");
        assert_eq!(names, vec!["svc-explicit"]);
    }

    #[test]
    fn given_protocol_version__when_protocol_compatibility_announcement__then_supported_is_none_and_unsupported_is_some()
     {
        assert!(
            protocol_compatibility_announcement("0.1.0").is_none(),
            "supported version should not emit announcement"
        );
        let unsupported = protocol_compatibility_announcement("0.2.0")
            .expect("unsupported version should emit announcement");
        assert!(unsupported.contains("not supported"));

        let invalid = protocol_compatibility_announcement("not-semver")
            .expect("invalid version should emit announcement");
        assert!(invalid.contains("invalid"));
    }

    #[test]
    fn given_command_start_ids__when_ids_match_or_mismatch__then_validation_matches_contract() {
        let request_id = Uuid::new_v4();
        ensure_command_start_request_id_match(request_id, request_id)
            .expect("matching request ids should pass");

        let err = ensure_command_start_request_id_match(request_id, Uuid::new_v4())
            .expect_err("mismatched request ids must fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, "command.start");
    }

    #[test]
    fn given_shutdown_flag__when_ensure_command_start_allowed__then_shutdown_is_rejected() {
        let accepting = AtomicBool::new(false);
        ensure_command_start_allowed(&accepting).expect("non-shutdown should pass");

        let shutting_down = AtomicBool::new(true);
        let err = ensure_command_start_allowed(&shutting_down)
            .expect_err("shutdown should reject command.start");
        assert_eq!(err.code, ErrorCode::Busy);
        assert_eq!(err.stage, "command.start");
        assert_eq!(err.message, "server is shutting down");
    }

    #[test]
    fn given_invalid_artifact_push_payload__when_validate_push_payload__then_bad_request_is_returned()
     {
        let payload = ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: "deploy-1".to_string(),
                offset: 0,
                length: 4,
                chunk_sha256: "abcd".to_string(),
                upload_token: "token-1".to_string(),
            },
            chunk: Vec::new(),
        };
        let err = validate_push_payload(&payload).expect_err("empty chunk should fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, "artifact.push");
    }

    #[tokio::test]
    async fn given_terminal_event_write_error__when_finalize_operation_after_terminal_event__then_operation_is_removed()
     {
        let operations = OperationManager::new();
        let request_id = Uuid::new_v4();
        operations
            .start(request_id, CommandType::Deploy)
            .await
            .expect("start should succeed");
        operations
            .set_state(&request_id, CommandState::Running, "running")
            .await
            .expect("set_state should succeed");

        let write_error =
            imagod_common::ImagodError::new(ErrorCode::Internal, "session.write", "stream closed");
        let result = finalize_operation_after_terminal_event(
            &operations,
            &request_id,
            CommandState::Failed,
            "failed",
            Err(write_error),
        )
        .await;

        assert!(result.is_err(), "write error should be returned");
        let snapshot = operations.snapshot_running(&request_id).await;
        assert!(
            snapshot.is_err(),
            "operation should be removed even on write error"
        );
    }
}
