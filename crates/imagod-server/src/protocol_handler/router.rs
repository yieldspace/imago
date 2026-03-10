use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use imagod_common::ImagodError;
use imagod_config::upsert_tls_known_client_key;
use imagod_spec::messages::{
    BindingsCertUploadRequest, BindingsCertUploadResponse, RpcInvokeRequest, RpcInvokeResponse,
};
use imagod_spec::{
    ArtifactPushRequest, CommandCancelRequest, CommandCancelResponse, CommandErrorKind,
    CommandEventType, CommandKind, CommandLifecycleState, CommandPayload, CommandProtocolAction,
    CommandProtocolContext, CommandProtocolOutput, CommandProtocolStageId, CommandStartRequest,
    CommandStartResponse, CommandState, CommandType, DeployPrepareRequest, ErrorCode, MessageType,
    PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSION_RANGE, ServiceListRequest, ServiceListResponse,
    StateRequest, StateResponse, Validate,
};
use semver::{Version, VersionReq};
use serde::Serialize;
use web_transport_quinn::SendStream;

use super::{
    Envelope, ProtocolHandler, ProtocolOperations,
    envelope_io::{bad_request, event_envelope, payload_take, response_envelope, write_envelope},
    logs_forwarder::LogsForwarder,
    session_loop::ProtocolSession,
    upsert_dynamic_client_public_key,
};

#[async_trait]
pub(crate) trait EnvelopeSink {
    async fn write(
        &mut self,
        handler: &ProtocolHandler,
        envelope: &Envelope,
    ) -> Result<(), ImagodError>;
}

pub(crate) struct SendStreamEnvelopeSink<'a> {
    send: &'a mut SendStream,
}

impl<'a> SendStreamEnvelopeSink<'a> {
    pub(crate) fn new(send: &'a mut SendStream) -> Self {
        Self { send }
    }
}

#[async_trait]
impl EnvelopeSink for SendStreamEnvelopeSink<'_> {
    async fn write(
        &mut self,
        handler: &ProtocolHandler,
        envelope: &Envelope,
    ) -> Result<(), ImagodError> {
        write_envelope(self.send, envelope, handler.frame_codec.as_ref()).await
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct RecordingEnvelopeSink {
    envelopes: Vec<Envelope>,
}

#[cfg(test)]
impl RecordingEnvelopeSink {
    #[allow(dead_code)]
    pub(crate) fn into_envelopes(self) -> Vec<Envelope> {
        self.envelopes
    }
}

#[cfg(test)]
#[async_trait]
impl EnvelopeSink for RecordingEnvelopeSink {
    async fn write(
        &mut self,
        _handler: &ProtocolHandler,
        envelope: &Envelope,
    ) -> Result<(), ImagodError> {
        self.envelopes.push(envelope.clone());
        Ok(())
    }
}

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
                ErrorCode::BadRequest,
                "dispatch",
                "unsupported message type",
            )),
        }
    }

    fn handle_hello(&self, mut request: Envelope) -> Result<Envelope, ImagodError> {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: imagod_spec::HelloNegotiateRequest = payload_take(&mut request)?;
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
            &imagod_spec::HelloNegotiateResponse {
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
        let payload: imagod_spec::ArtifactCommitRequest = payload_take(&mut request)?;
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
            .execute(
                &command_context(payload.request_id),
                &CommandProtocolAction::SnapshotRunning,
            )
            .await;
        let response = state_response_from_output(payload.request_id, response)?;
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

        let response = self
            .operations
            .execute(
                &command_context(payload.request_id),
                &CommandProtocolAction::RequestCancel,
            )
            .await;
        let response = cancel_response_from_output(response)?;
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
        request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError>
    where
        S: ProtocolSession + 'static,
    {
        let mut sink = SendStreamEnvelopeSink::new(send);
        self.handle_logs_request_with_sink(session, request, &mut sink)
            .await
    }

    pub(crate) async fn handle_logs_request_with_sink<S>(
        &self,
        session: Arc<S>,
        mut request: Envelope,
        sink: &mut (impl EnvelopeSink + ?Sized),
    ) -> Result<(), ImagodError>
    where
        S: ProtocolSession + 'static,
    {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        let payload: imagod_spec::LogRequest = payload_take(&mut request)?;
        payload
            .validate()
            .map_err(|e| bad_request("logs.request", e.to_string()))?;
        let imagod_spec::LogRequest {
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
                .open_logs(service_name, tail_lines, follow, with_timestamp)
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
        sink.write(self, &ack).await?;

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
        request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError> {
        let mut sink = SendStreamEnvelopeSink::new(send);
        self.handle_command_start_with_sink(request, &mut sink)
            .await
    }

    pub(crate) async fn handle_command_start_with_sink(
        &self,
        mut request: Envelope,
        sink: &mut (impl EnvelopeSink + ?Sized),
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
        let operation_context = command_context(operation_id);
        let deploy_id_for_cleanup = if payload.command_type == CommandType::Deploy {
            match &payload.payload {
                CommandPayload::Deploy(deploy_payload) => Some(deploy_payload.deploy_id.clone()),
                _ => None,
            }
        } else {
            None
        };

        expect_ack(
            self.operations
                .execute(
                    &operation_context,
                    &CommandProtocolAction::Start(command_kind_from_wire(payload.command_type)),
                )
                .await,
            CommandProtocolStageId::CommandStart,
        )?;

        let accepted = response_envelope(
            MessageType::CommandStart,
            request_id,
            correlation_id,
            &CommandStartResponse { accepted: true },
        )?;
        sink.write(self, &accepted).await?;

        let accepted_event = event_envelope(
            self.clock.as_ref(),
            operation_id,
            correlation_id,
            CommandEventType::Accepted,
            payload.command_type,
            None,
            None,
        )?;
        sink.write(self, &accepted_event).await?;

        expect_ack(
            self.operations
                .execute(&operation_context, &CommandProtocolAction::SetRunning)
                .await,
            CommandProtocolStageId::OperationState,
        )?;
        let running_event = event_envelope(
            self.clock.as_ref(),
            operation_id,
            correlation_id,
            CommandEventType::Progress,
            payload.command_type,
            Some("starting".to_string()),
            None,
        )?;
        sink.write(self, &running_event).await?;

        let spawn_transition = self
            .operations
            .execute(&operation_context, &CommandProtocolAction::MarkSpawned)
            .await;
        if is_spawn_canceled(&spawn_transition)? {
            let canceled = event_envelope(
                self.clock.as_ref(),
                operation_id,
                correlation_id,
                CommandEventType::Canceled,
                payload.command_type,
                Some("canceled".to_string()),
                None,
            )?;
            let canceled_write = sink.write(self, &canceled).await;
            let finalize_result = finalize_operation_after_terminal_event(
                self.operations.as_ref(),
                &operation_context,
                CommandProtocolAction::FinishCanceled,
                canceled_write,
            )
            .await;
            self.purge_deploy_session_if_needed(
                deploy_id_for_cleanup.as_deref(),
                CommandState::Canceled,
                None,
            )
            .await;
            finalize_result?;
            return Ok(());
        }

        let command_result = match (&payload.command_type, &payload.payload) {
            (CommandType::Deploy, CommandPayload::Deploy(deploy_payload)) => {
                self.orchestrator.command_deploy(deploy_payload).await
            }
            (CommandType::Run, CommandPayload::Run(run_payload)) => {
                self.orchestrator.command_run(run_payload).await
            }
            (CommandType::Stop, CommandPayload::Stop(stop_payload)) => {
                self.orchestrator.command_stop(stop_payload).await
            }
            _ => Err(ImagodError::new(
                ErrorCode::BadRequest,
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
                sink.write(self, &progress).await?;

                let succeeded = event_envelope(
                    self.clock.as_ref(),
                    operation_id,
                    correlation_id,
                    CommandEventType::Succeeded,
                    payload.command_type,
                    Some(success_stage_for_event),
                    None,
                )?;
                let succeeded_write = sink.write(self, &succeeded).await;
                let finalize_result = finalize_operation_after_terminal_event(
                    self.operations.as_ref(),
                    &operation_context,
                    CommandProtocolAction::FinishSucceeded,
                    succeeded_write,
                )
                .await;
                self.purge_deploy_session_if_needed(
                    deploy_id_for_cleanup.as_deref(),
                    CommandState::Succeeded,
                    None,
                )
                .await;
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
                let failed_write = sink.write(self, &failed).await;
                let finalize_result = finalize_operation_after_terminal_event(
                    self.operations.as_ref(),
                    &operation_context,
                    CommandProtocolAction::FinishFailed(command_error_kind_from_wire(err.code)),
                    failed_write,
                )
                .await;
                self.purge_deploy_session_if_needed(
                    deploy_id_for_cleanup.as_deref(),
                    CommandState::Failed,
                    Some(&err),
                )
                .await;
                finalize_result?;
            }
        }

        Ok(())
    }

    async fn purge_deploy_session_if_needed(
        &self,
        deploy_id: Option<&str>,
        terminal_state: CommandState,
        terminal_error: Option<&ImagodError>,
    ) {
        let Some(deploy_id) = deploy_id else {
            return;
        };
        if !should_purge_deploy_session_after_terminal(terminal_state, terminal_error) {
            return;
        }
        if let Err(err) = self.artifacts.purge_deploy_session(deploy_id).await {
            eprintln!(
                "artifact session purge failed deploy_id={} code={:?} stage={} message={}",
                deploy_id, err.code, err.stage, err.message
            );
        }
    }
}

fn command_kind_from_wire(command_type: CommandType) -> CommandKind {
    match command_type {
        CommandType::Deploy => CommandKind::Deploy,
        CommandType::Run => CommandKind::Run,
        CommandType::Stop => CommandKind::Stop,
    }
}

fn command_state_to_wire(state: CommandLifecycleState) -> CommandState {
    match state {
        CommandLifecycleState::Accepted => CommandState::Accepted,
        CommandLifecycleState::Running => CommandState::Running,
        CommandLifecycleState::Succeeded => CommandState::Succeeded,
        CommandLifecycleState::Failed => CommandState::Failed,
        CommandLifecycleState::Canceled => CommandState::Canceled,
    }
}

fn command_error_kind_from_wire(code: ErrorCode) -> CommandErrorKind {
    match code {
        ErrorCode::Unauthorized => CommandErrorKind::Unauthorized,
        ErrorCode::BadRequest => CommandErrorKind::BadRequest,
        ErrorCode::BadManifest => CommandErrorKind::BadManifest,
        ErrorCode::Busy => CommandErrorKind::Busy,
        ErrorCode::NotFound => CommandErrorKind::NotFound,
        ErrorCode::Internal => CommandErrorKind::Internal,
        ErrorCode::IdempotencyConflict => CommandErrorKind::IdempotencyConflict,
        ErrorCode::RangeInvalid => CommandErrorKind::RangeInvalid,
        ErrorCode::ChunkHashMismatch => CommandErrorKind::ChunkHashMismatch,
        ErrorCode::ArtifactIncomplete => CommandErrorKind::ArtifactIncomplete,
        ErrorCode::PreconditionFailed => CommandErrorKind::PreconditionFailed,
        ErrorCode::OperationTimeout => CommandErrorKind::OperationTimeout,
        ErrorCode::RollbackFailed => CommandErrorKind::RollbackFailed,
        ErrorCode::StorageQuota => CommandErrorKind::StorageQuota,
    }
}

fn command_error_kind_to_wire(code: CommandErrorKind) -> ErrorCode {
    match code {
        CommandErrorKind::Unauthorized => ErrorCode::Unauthorized,
        CommandErrorKind::BadRequest => ErrorCode::BadRequest,
        CommandErrorKind::BadManifest => ErrorCode::BadManifest,
        CommandErrorKind::Busy => ErrorCode::Busy,
        CommandErrorKind::NotFound => ErrorCode::NotFound,
        CommandErrorKind::Internal => ErrorCode::Internal,
        CommandErrorKind::IdempotencyConflict => ErrorCode::IdempotencyConflict,
        CommandErrorKind::RangeInvalid => ErrorCode::RangeInvalid,
        CommandErrorKind::ChunkHashMismatch => ErrorCode::ChunkHashMismatch,
        CommandErrorKind::ArtifactIncomplete => ErrorCode::ArtifactIncomplete,
        CommandErrorKind::PreconditionFailed => ErrorCode::PreconditionFailed,
        CommandErrorKind::OperationTimeout => ErrorCode::OperationTimeout,
        CommandErrorKind::RollbackFailed => ErrorCode::RollbackFailed,
        CommandErrorKind::StorageQuota => ErrorCode::StorageQuota,
    }
}

fn command_context(request_id: uuid::Uuid) -> CommandProtocolContext {
    CommandProtocolContext { request_id }
}

fn command_rejection_message(
    stage: CommandProtocolStageId,
    code: CommandErrorKind,
) -> &'static str {
    match (stage, code) {
        (CommandProtocolStageId::CommandStart, CommandErrorKind::Busy) => {
            "request_id is already running"
        }
        (CommandProtocolStageId::StateRequest, CommandErrorKind::NotFound)
        | (CommandProtocolStageId::CommandCancel, CommandErrorKind::NotFound)
        | (CommandProtocolStageId::OperationState, CommandErrorKind::NotFound)
        | (CommandProtocolStageId::OperationRemove, CommandErrorKind::NotFound) => {
            "request_id is not running"
        }
        _ => "command action rejected",
    }
}

fn imagod_error_from_rejected_output(
    code: CommandErrorKind,
    stage: CommandProtocolStageId,
) -> ImagodError {
    ImagodError::new(
        command_error_kind_to_wire(code),
        stage.as_wire(),
        command_rejection_message(stage, code),
    )
}

fn state_response_from_output(
    request_id: uuid::Uuid,
    output: CommandProtocolOutput,
) -> Result<StateResponse, ImagodError> {
    match output {
        CommandProtocolOutput::StateSnapshot {
            state,
            stage,
            updated_at_unix_secs,
        } => Ok(StateResponse {
            request_id,
            state: command_state_to_wire(state),
            stage,
            updated_at: updated_at_unix_secs.to_string(),
        }),
        CommandProtocolOutput::Rejected { code, stage } => {
            Err(imagod_error_from_rejected_output(code, stage))
        }
        other => Err(ImagodError::new(
            ErrorCode::Internal,
            "state.request",
            format!("unexpected state.request output: {other:?}"),
        )),
    }
}

fn cancel_response_from_output(
    output: CommandProtocolOutput,
) -> Result<CommandCancelResponse, ImagodError> {
    match output {
        CommandProtocolOutput::CancelResponse {
            cancellable,
            final_state,
        } => Ok(CommandCancelResponse {
            cancellable,
            final_state: command_state_to_wire(final_state),
        }),
        CommandProtocolOutput::Rejected { code, stage } => {
            Err(imagod_error_from_rejected_output(code, stage))
        }
        other => Err(ImagodError::new(
            ErrorCode::Internal,
            "command.cancel",
            format!("unexpected command.cancel output: {other:?}"),
        )),
    }
}

fn expect_ack(
    output: CommandProtocolOutput,
    expected_stage: CommandProtocolStageId,
) -> Result<(), ImagodError> {
    match output {
        CommandProtocolOutput::Ack => Ok(()),
        CommandProtocolOutput::Rejected { code, stage } => {
            Err(imagod_error_from_rejected_output(code, stage))
        }
        other => Err(ImagodError::new(
            ErrorCode::Internal,
            expected_stage.as_wire(),
            format!("unexpected ack output: {other:?}"),
        )),
    }
}

fn is_spawn_canceled(output: &CommandProtocolOutput) -> Result<bool, ImagodError> {
    match output {
        CommandProtocolOutput::SpawnResult { spawned, canceled } => Ok(!spawned && *canceled),
        CommandProtocolOutput::Rejected { code, stage } => {
            Err(imagod_error_from_rejected_output(*code, *stage))
        }
        other => Err(ImagodError::new(
            ErrorCode::Internal,
            "operation.state",
            format!("unexpected spawn output: {other:?}"),
        )),
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
        ErrorCode::Busy,
        "command.start",
        "server is shutting down",
    ))
}

pub(crate) fn validate_push_payload(payload: &ArtifactPushRequest) -> Result<(), ImagodError> {
    payload
        .validate()
        .map_err(|e| bad_request("artifact.push", e.to_string()))
}

fn should_purge_deploy_session_after_terminal(
    terminal_state: CommandState,
    terminal_error: Option<&ImagodError>,
) -> bool {
    match terminal_state {
        CommandState::Succeeded => true,
        CommandState::Canceled => false,
        CommandState::Failed => {
            terminal_error.is_some_and(|err| err.code != ErrorCode::Busy && !err.retryable)
        }
        _ => false,
    }
}

fn resolve_logs_request_service_names(
    requested_name: Option<String>,
    loggable_names: Vec<String>,
) -> Result<Vec<String>, ImagodError> {
    match requested_name {
        Some(name) => Ok(vec![name]),
        None if !loggable_names.is_empty() => Ok(loggable_names),
        None => Err(ImagodError::new(
            ErrorCode::NotFound,
            "logs.request",
            "no loggable services are available",
        )),
    }
}

/// Finalizes operation bookkeeping after writing terminal event.
pub(crate) async fn finalize_operation_after_terminal_event(
    operations: &(impl ProtocolOperations + ?Sized),
    context: &CommandProtocolContext,
    terminal_action: CommandProtocolAction,
    terminal_write_result: Result<(), ImagodError>,
) -> Result<(), ImagodError> {
    expect_ack(
        operations.execute(context, &terminal_action).await,
        CommandProtocolStageId::OperationState,
    )?;
    expect_ack(
        operations
            .execute(context, &CommandProtocolAction::Remove)
            .await,
        CommandProtocolStageId::OperationRemove,
    )?;
    terminal_write_result
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    use std::sync::atomic::AtomicBool;

    use imagod_control::{ActionApplier, OperationManager};
    use imagod_spec::{
        ArtifactPushChunkHeader, ArtifactPushRequest, CommandErrorKind, CommandKind,
        CommandLifecycleState, CommandProtocolAction, CommandProtocolContext,
        CommandProtocolOutput, CommandProtocolStageId, CommandState, CommandType,
    };
    use uuid::Uuid;

    use super::{
        cancel_response_from_output, command_error_kind_from_wire, command_error_kind_to_wire,
        command_kind_from_wire, command_state_to_wire, ensure_command_start_allowed,
        ensure_command_start_request_id_match, finalize_operation_after_terminal_event,
        protocol_compatibility_announcement, resolve_logs_request_service_names,
        should_purge_deploy_session_after_terminal, state_response_from_output,
        validate_push_payload,
    };
    use imagod_spec::ErrorCode;

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
    fn given_wire_command_values__when_converted_to_model__then_mapping_is_exhaustive() {
        assert_eq!(
            command_kind_from_wire(CommandType::Deploy),
            CommandKind::Deploy
        );
        assert_eq!(command_kind_from_wire(CommandType::Run), CommandKind::Run);
        assert_eq!(command_kind_from_wire(CommandType::Stop), CommandKind::Stop);

        assert_eq!(
            command_state_to_wire(CommandLifecycleState::Accepted),
            CommandState::Accepted
        );
        assert_eq!(
            command_state_to_wire(CommandLifecycleState::Running),
            CommandState::Running
        );
        assert_eq!(
            command_state_to_wire(CommandLifecycleState::Succeeded),
            CommandState::Succeeded
        );
        assert_eq!(
            command_state_to_wire(CommandLifecycleState::Failed),
            CommandState::Failed
        );
        assert_eq!(
            command_state_to_wire(CommandLifecycleState::Canceled),
            CommandState::Canceled
        );

        assert_eq!(
            command_error_kind_from_wire(ErrorCode::Internal),
            CommandErrorKind::Internal
        );
        assert_eq!(
            command_error_kind_from_wire(ErrorCode::Busy),
            CommandErrorKind::Busy
        );
        assert_eq!(
            command_error_kind_from_wire(ErrorCode::NotFound),
            CommandErrorKind::NotFound
        );
        assert_eq!(
            command_error_kind_to_wire(CommandErrorKind::Internal),
            ErrorCode::Internal
        );
    }

    #[test]
    fn given_internal_operation_views__when_wrapped_for_wire__then_protocol_contract_is_preserved()
    {
        let state = state_response_from_output(
            Uuid::from_u128(1),
            CommandProtocolOutput::StateSnapshot {
                state: CommandLifecycleState::Running,
                stage: "running".to_string(),
                updated_at_unix_secs: 1,
            },
        )
        .expect("snapshot output should convert");
        assert_eq!(state.state, CommandState::Running);
        assert_eq!(state.stage, "running");
        assert_eq!(state.updated_at, "1");

        let cancel = cancel_response_from_output(CommandProtocolOutput::CancelResponse {
            cancellable: true,
            final_state: CommandLifecycleState::Canceled,
        })
        .expect("cancel output should convert");
        assert!(cancel.cancellable);
        assert_eq!(cancel.final_state, CommandState::Canceled);
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

    #[test]
    fn given_terminal_result__when_should_purge_deploy_session_after_terminal__then_policy_matches_contract()
     {
        assert!(
            should_purge_deploy_session_after_terminal(CommandState::Succeeded, None),
            "succeeded deploy should purge committed artifact session"
        );
        assert!(
            !should_purge_deploy_session_after_terminal(CommandState::Canceled, None),
            "canceled deploy should keep committed artifact session for retry"
        );

        let retryable_err =
            imagod_common::ImagodError::new(ErrorCode::Internal, "orchestration", "retryable")
                .with_retryable(true);
        assert!(
            !should_purge_deploy_session_after_terminal(CommandState::Failed, Some(&retryable_err)),
            "retryable failure should keep committed artifact session for retry"
        );

        let busy_non_retryable =
            imagod_common::ImagodError::new(ErrorCode::Busy, "command.start", "busy")
                .with_retryable(false);
        assert!(
            !should_purge_deploy_session_after_terminal(
                CommandState::Failed,
                Some(&busy_non_retryable)
            ),
            "busy failure should keep committed artifact session for retry"
        );

        let non_retryable_err =
            imagod_common::ImagodError::new(ErrorCode::Internal, "orchestration", "fatal")
                .with_retryable(false);
        assert!(
            should_purge_deploy_session_after_terminal(
                CommandState::Failed,
                Some(&non_retryable_err)
            ),
            "non-retryable failure should purge committed artifact session"
        );
    }

    #[tokio::test]
    async fn given_terminal_event_write_error__when_finalize_operation_after_terminal_event__then_operation_is_removed()
     {
        let operations = OperationManager::new();
        let request_id = Uuid::new_v4();
        <OperationManager as ActionApplier>::execute_action(
            &operations,
            &CommandProtocolContext { request_id },
            &CommandProtocolAction::Start(CommandKind::Deploy),
        )
        .await;
        <OperationManager as ActionApplier>::execute_action(
            &operations,
            &CommandProtocolContext { request_id },
            &CommandProtocolAction::SetRunning,
        )
        .await;
        <OperationManager as ActionApplier>::execute_action(
            &operations,
            &CommandProtocolContext { request_id },
            &CommandProtocolAction::MarkSpawned,
        )
        .await;

        let write_error =
            imagod_common::ImagodError::new(ErrorCode::Internal, "session.write", "stream closed");
        let result = finalize_operation_after_terminal_event(
            &operations,
            &CommandProtocolContext { request_id },
            CommandProtocolAction::FinishFailed(CommandErrorKind::Internal),
            Err(write_error),
        )
        .await;

        assert!(result.is_err(), "write error should be returned");
        let snapshot = <OperationManager as ActionApplier>::execute_action(
            &operations,
            &CommandProtocolContext { request_id },
            &CommandProtocolAction::SnapshotRunning,
        )
        .await;
        assert!(
            matches!(
                snapshot,
                CommandProtocolOutput::Rejected {
                    code: CommandErrorKind::NotFound,
                    stage: CommandProtocolStageId::StateRequest,
                }
            ),
            "operation should be removed even on write error"
        );
    }
}

#[cfg(test)]
mod conformance_tests {
    use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

    use async_trait::async_trait;
    use imagod_config::{RuntimeConfig, TlsConfig, load_or_create_default};
    use imagod_control::{ActionApplier, OperationManager, ServiceLogSubscription};
    use imagod_spec::{
        ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushChunkHeader,
        ArtifactPushRequest, ArtifactStatus, BindingsCertUploadRequest, ByteRange,
        CommandCancelRequest, CommandKind, CommandProtocolAction, CommandProtocolContext,
        CommandProtocolOutput, ContractEffectSummary, DeployCommandPayload, DeployPrepareRequest,
        DeployPrepareResponse, ErrorCode, HelloNegotiateRequest, MessageType, RouterOutputSummary,
        RouterStateSummary, RpcInvokeRequest, RpcInvokeTargetService, RunCommandPayload,
        ServiceListRequest, ServiceState, ServiceStatusEntry, StateRequest, StopCommandPayload,
        SummaryRequestKind, SummaryStreamId,
    };
    use imagod_spec_formal::{
        RouterProjectionAction, RouterProjectionSpec, SystemMessageBinding, system_message_binding,
    };
    use nirvash_macros::nirvash_runtime_contract;
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        super::{ProtocolArtifacts, ProtocolOrchestrator},
        Envelope, ProtocolHandler, ProtocolOperations,
    };

    struct RouterRuntime {
        handler: ProtocolHandler,
        summary: tokio::sync::Mutex<RouterStateSummary>,
        _config_root: PathBuf,
    }

    struct FakeArtifacts;

    #[async_trait]
    impl ProtocolArtifacts for FakeArtifacts {
        async fn prepare(
            &self,
            _payload: DeployPrepareRequest,
        ) -> Result<DeployPrepareResponse, imagod_common::ImagodError> {
            Ok(DeployPrepareResponse {
                deploy_id: "deploy-1".to_string(),
                artifact_status: ArtifactStatus::Partial,
                missing_ranges: vec![ByteRange {
                    offset: 0,
                    length: 4,
                }],
                upload_token: "token-1".to_string(),
                session_expires_at: "1735689600".to_string(),
            })
        }

        async fn push(
            &self,
            _payload: ArtifactPushRequest,
        ) -> Result<ArtifactPushAck, imagod_common::ImagodError> {
            Ok(ArtifactPushAck {
                received_ranges: vec![ByteRange {
                    offset: 0,
                    length: 4,
                }],
                next_missing_range: None,
                accepted_bytes: 4,
            })
        }

        async fn commit(
            &self,
            _payload: ArtifactCommitRequest,
        ) -> Result<ArtifactCommitResponse, imagod_common::ImagodError> {
            Ok(ArtifactCommitResponse {
                artifact_id: "artifact-1".to_string(),
                verified: true,
            })
        }

        async fn purge_deploy_session(
            &self,
            _deploy_id: &str,
        ) -> Result<(), imagod_common::ImagodError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FakeOperations {
        manager: Arc<OperationManager>,
    }

    #[async_trait]
    impl ProtocolOperations for FakeOperations {
        async fn execute(
            &self,
            context: &CommandProtocolContext,
            action: &CommandProtocolAction,
        ) -> CommandProtocolOutput {
            self.manager.execute_action(context, action).await
        }
    }

    struct FakeOrchestrator;

    #[async_trait]
    impl ProtocolOrchestrator for FakeOrchestrator {
        async fn command_deploy(
            &self,
            _payload: &DeployCommandPayload,
        ) -> Result<(String, String), imagod_common::ImagodError> {
            Ok(("release:svc-a:release-a".to_string(), "spawned".to_string()))
        }

        async fn command_run(
            &self,
            _payload: &RunCommandPayload,
        ) -> Result<(String, String), imagod_common::ImagodError> {
            Ok(("running:svc-a:release-a".to_string(), "spawned".to_string()))
        }

        async fn command_stop(
            &self,
            _payload: &StopCommandPayload,
        ) -> Result<(String, String), imagod_common::ImagodError> {
            Ok(("stopped:svc-a".to_string(), "completed".to_string()))
        }

        async fn list_service_states(
            &self,
            _names_filter: Option<&[String]>,
        ) -> Result<Vec<ServiceStatusEntry>, imagod_common::ImagodError> {
            Ok(vec![ServiceStatusEntry {
                name: "svc-a".to_string(),
                release_hash: "release-a".to_string(),
                started_at: "1735689600".to_string(),
                state: ServiceState::Running,
            }])
        }

        async fn loggable_service_names(&self) -> Vec<String> {
            vec!["svc-a".to_string()]
        }

        async fn open_logs(
            &self,
            _service_name: &str,
            _tail_lines: u32,
            _follow: bool,
            _with_timestamp: bool,
        ) -> Result<ServiceLogSubscription, imagod_common::ImagodError> {
            Err(imagod_common::ImagodError::new(
                ErrorCode::Internal,
                "logs.request",
                "router projection does not open logs",
            ))
        }

        async fn invoke(
            &self,
            _target_service_name: &str,
            _interface_id: &str,
            _function: &str,
            _args_cbor: Vec<u8>,
        ) -> Result<Vec<u8>, imagod_common::ImagodError> {
            Ok(vec![0x2a])
        }

        async fn reap_finished_services(&self) {}

        async fn has_live_services(&self) -> bool {
            true
        }

        async fn stop_all_services(
            &self,
            _force: bool,
        ) -> Vec<(String, imagod_common::ImagodError)> {
            Vec::new()
        }
    }

    impl RouterRuntime {
        async fn new() -> Self {
            let config_root =
                std::env::temp_dir().join(format!("imagod-router-projection-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&config_root).expect("config root should exist");
            let config_path = config_root.join("imagod.toml");
            let loaded = load_or_create_default(&config_path).expect("default config should load");
            let mut config = loaded.config;
            config.server_version = "imagod/test".to_string();
            config.runtime = RuntimeConfig::default();
            config.listen_addr = "127.0.0.1:4443".to_string();
            if config.tls.server_key.as_os_str().is_empty() {
                config.tls = TlsConfig {
                    server_key: PathBuf::new(),
                    admin_public_keys: Vec::new(),
                    client_public_keys: Vec::new(),
                    known_public_keys: BTreeMap::new(),
                };
            }

            let manager = Arc::new(OperationManager::new());
            manager
                .execute_action(
                    &CommandProtocolContext {
                        request_id: Uuid::from_u128(7),
                    },
                    &CommandProtocolAction::Start(CommandKind::Deploy),
                )
                .await;
            let handler = ProtocolHandler::new_with_clients(
                Arc::new(config),
                config_path,
                Arc::new(FakeArtifacts),
                Arc::new(FakeOperations {
                    manager: manager.clone(),
                }),
                Arc::new(FakeOrchestrator),
                Arc::new(crate::protocol_handler::codec::LengthPrefixedFrameCodec),
                Arc::new(crate::protocol_handler::clock::SystemServerClock),
                Arc::new(crate::protocol_handler::logs_forwarder::DefaultLogsForwarder),
            );
            Self {
                handler,
                summary: tokio::sync::Mutex::new(RouterStateSummary::default()),
                _config_root: config_root,
            }
        }

        fn request_for(&self, action: RouterProjectionAction) -> Envelope {
            let request_id = Uuid::from_u128(7);
            let correlation_id = Uuid::from_u128(2);
            match action {
                RouterProjectionAction::HelloNegotiate => Envelope::new(
                    MessageType::HelloNegotiate,
                    request_id,
                    correlation_id,
                    json!(HelloNegotiateRequest {
                        client_version: "0.1.0".to_string(),
                        required_features: Vec::new(),
                    }),
                ),
                RouterProjectionAction::DeployPrepare => Envelope::new(
                    MessageType::DeployPrepare,
                    request_id,
                    correlation_id,
                    json!(DeployPrepareRequest {
                        name: "svc-a".to_string(),
                        app_type: "rpc".to_string(),
                        target: BTreeMap::new(),
                        artifact_digest: "sha256:artifact".to_string(),
                        artifact_size: 4,
                        manifest_digest: "sha256:manifest".to_string(),
                        idempotency_key: "deploy-1".to_string(),
                        policy: BTreeMap::new(),
                    }),
                ),
                RouterProjectionAction::ArtifactPush => Envelope::new(
                    MessageType::ArtifactPush,
                    request_id,
                    correlation_id,
                    json!(ArtifactPushRequest {
                        header: ArtifactPushChunkHeader {
                            deploy_id: "deploy-1".to_string(),
                            offset: 0,
                            length: 4,
                            chunk_sha256: "abcd".to_string(),
                            upload_token: "token-1".to_string(),
                        },
                        chunk: vec![1, 2, 3, 4],
                    }),
                ),
                RouterProjectionAction::ArtifactCommit => Envelope::new(
                    MessageType::ArtifactCommit,
                    request_id,
                    correlation_id,
                    json!(ArtifactCommitRequest {
                        deploy_id: "deploy-1".to_string(),
                        artifact_digest: "sha256:artifact".to_string(),
                        artifact_size: 4,
                        manifest_digest: "sha256:manifest".to_string(),
                    }),
                ),
                RouterProjectionAction::StateRequest => Envelope::new(
                    MessageType::StateRequest,
                    request_id,
                    correlation_id,
                    json!(StateRequest {
                        request_id: Uuid::from_u128(7),
                    }),
                ),
                RouterProjectionAction::ServicesList => Envelope::new(
                    MessageType::ServicesList,
                    request_id,
                    correlation_id,
                    json!(ServiceListRequest { names: None }),
                ),
                RouterProjectionAction::CommandCancel => Envelope::new(
                    MessageType::CommandCancel,
                    request_id,
                    correlation_id,
                    json!(CommandCancelRequest { request_id }),
                ),
                RouterProjectionAction::RpcInvoke => Envelope::new(
                    MessageType::RpcInvoke,
                    request_id,
                    correlation_id,
                    json!(RpcInvokeRequest {
                        interface_id: "yieldspace:service/invoke".to_string(),
                        function: "call".to_string(),
                        args_cbor: vec![0x01],
                        target_service: RpcInvokeTargetService {
                            name: "svc-a".to_string(),
                        },
                    }),
                ),
                RouterProjectionAction::BindingsCertUpload => Envelope::new(
                    MessageType::BindingsCertUpload,
                    request_id,
                    correlation_id,
                    json!(BindingsCertUploadRequest {
                        authority: "edge.example".to_string(),
                        public_key_hex:
                            "1111111111111111111111111111111111111111111111111111111111111111"
                                .to_string(),
                    }),
                ),
            }
        }

        fn output_for_response(&self, response: &Envelope) -> RouterOutputSummary {
            match system_message_binding(response.message_type) {
                SystemMessageBinding::Request(kind) | SystemMessageBinding::Response(kind) => {
                    RouterOutputSummary {
                        effects: vec![ContractEffectSummary::Response(
                            SummaryStreamId::Stream0,
                            summary_request_kind(kind),
                        )],
                    }
                }
                SystemMessageBinding::CommandEvent
                | SystemMessageBinding::LogChunk
                | SystemMessageBinding::LogsEnd => RouterOutputSummary::default(),
            }
        }

        async fn dispatch(
            &self,
            action: RouterProjectionAction,
        ) -> Result<Envelope, imagod_common::ImagodError> {
            let request = self.request_for(action);
            self.handler.handle_single(request).await
        }
    }

    fn summary_request_kind(
        kind: imagod_spec_formal::atoms::RequestKindAtom,
    ) -> SummaryRequestKind {
        match kind {
            imagod_spec_formal::atoms::RequestKindAtom::HelloNegotiate => {
                SummaryRequestKind::HelloNegotiate
            }
            imagod_spec_formal::atoms::RequestKindAtom::DeployPrepare => {
                SummaryRequestKind::DeployPrepare
            }
            imagod_spec_formal::atoms::RequestKindAtom::ArtifactPush => {
                SummaryRequestKind::ArtifactPush
            }
            imagod_spec_formal::atoms::RequestKindAtom::ArtifactCommit => {
                SummaryRequestKind::ArtifactCommit
            }
            imagod_spec_formal::atoms::RequestKindAtom::CommandStart => {
                SummaryRequestKind::CommandStart
            }
            imagod_spec_formal::atoms::RequestKindAtom::StateRequest => {
                SummaryRequestKind::StateRequest
            }
            imagod_spec_formal::atoms::RequestKindAtom::ServicesList => {
                SummaryRequestKind::ServicesList
            }
            imagod_spec_formal::atoms::RequestKindAtom::CommandCancel => {
                SummaryRequestKind::CommandCancel
            }
            imagod_spec_formal::atoms::RequestKindAtom::LogsRequest => {
                SummaryRequestKind::LogsRequest
            }
            imagod_spec_formal::atoms::RequestKindAtom::RpcInvoke => SummaryRequestKind::RpcInvoke,
            imagod_spec_formal::atoms::RequestKindAtom::BindingsCertUpload => {
                SummaryRequestKind::BindingsCertUpload
            }
            other => panic!("unexpected router request kind: {other:?}"),
        }
    }

    fn router_output(
        runtime: &RouterRuntime,
        result: &Result<Envelope, imagod_common::ImagodError>,
    ) -> RouterOutputSummary {
        runtime.output_for_response(result.as_ref().expect("router response should exist"))
    }

    fn router_law_output(kind: SummaryRequestKind) -> RouterOutputSummary {
        RouterOutputSummary {
            effects: vec![ContractEffectSummary::Response(
                SummaryStreamId::Stream0,
                kind,
            )],
        }
    }

    #[nirvash_runtime_contract(
        spec = RouterProjectionSpec,
        binding = RouterProjectionBinding,
        context = (),
        context_expr = (),
        summary = RouterStateSummary,
        output = RouterOutputSummary,
        summary_field = summary,
        initial_summary = RouterStateSummary::initial_admin_stream(),
        fresh_runtime = RouterRuntime::new().await,
        tests(grouped)
    )]
    impl RouterRuntime {
        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::HelloNegotiate,
            requires = summary.active_session && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::HelloNegotiate)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::HelloNegotiate)
        )]
        async fn contract_hello_negotiate(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::HelloNegotiate).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::DeployPrepare,
            requires = summary.active_session
                && summary.deploy_prepare_authorized
                && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::DeployPrepare)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::DeployPrepare)
        )]
        async fn contract_deploy_prepare(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::DeployPrepare).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::ArtifactPush,
            requires = summary.active_session
                && summary.artifact_push_authorized
                && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::ArtifactPush)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::ArtifactPush)
        )]
        async fn contract_artifact_push(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::ArtifactPush).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::ArtifactCommit,
            requires = summary.active_session
                && summary.artifact_commit_authorized
                && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::ArtifactCommit)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::ArtifactCommit)
        )]
        async fn contract_artifact_commit(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::ArtifactCommit).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::StateRequest,
            requires = summary.active_session
                && summary.state_request_authorized
                && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::StateRequest)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::StateRequest)
        )]
        async fn contract_state_request(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::StateRequest).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::ServicesList,
            requires = summary.active_session
                && summary.services_list_authorized
                && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::ServicesList)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::ServicesList)
        )]
        async fn contract_services_list(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::ServicesList).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::CommandCancel,
            requires = summary.active_session
                && summary.command_cancel_authorized
                && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::CommandCancel)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::CommandCancel)
        )]
        async fn contract_command_cancel(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::CommandCancel).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::RpcInvoke,
            requires = summary.active_session
                && summary.rpc_invoke_authorized
                && summary.request.is_none(),
            update(request = Some(SummaryRequestKind::RpcInvoke)),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::RpcInvoke)
        )]
        async fn contract_rpc_invoke(&self) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::RpcInvoke).await
        }

        #[nirvash_macros::contract_case(
            action = RouterProjectionAction::BindingsCertUpload,
            requires = summary.active_session
                && summary.bindings_cert_upload_authorized
                && summary.request.is_none(),
            update(
                request = Some(SummaryRequestKind::BindingsCertUpload),
                authority_uploaded = true
            ),
            output = |result| router_output(self, result),
            law_output = router_law_output(SummaryRequestKind::BindingsCertUpload)
        )]
        async fn contract_bindings_cert_upload(
            &self,
        ) -> Result<Envelope, imagod_common::ImagodError> {
            self.dispatch(RouterProjectionAction::BindingsCertUpload)
                .await
        }
    }

    #[test]
    fn router_runtime_maps_services_list_to_system_response() {
        let spec = RouterProjectionSpec::new();
        let runtime: RouterRuntime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build")
            .block_on(async {
                <RouterProjectionBinding as nirvash_core::conformance::ProtocolRuntimeBinding<
                    RouterProjectionSpec,
                >>::fresh_runtime(&spec)
                .await
            });
        let output = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build")
            .block_on(async {
                <RouterRuntime as nirvash_core::conformance::ActionApplier>::execute_action(
                    &runtime,
                    &(),
                    &RouterProjectionAction::ServicesList,
                )
                .await
            });
        assert_eq!(
            output,
            RouterOutputSummary {
                effects: vec![ContractEffectSummary::Response(
                    SummaryStreamId::Stream0,
                    SummaryRequestKind::ServicesList,
                )],
            }
        );
    }
}
