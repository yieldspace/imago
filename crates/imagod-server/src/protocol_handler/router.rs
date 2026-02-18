use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use imago_protocol::{
    ArtifactPushRequest, CommandCancelRequest, CommandEventType, CommandPayload,
    CommandStartRequest, CommandStartResponse, CommandState, CommandType, DeployPrepareRequest,
    MessageType, StateRequest, Validate,
};
use imagod_common::ImagodError;
use imagod_control::{OperationManager, SpawnTransition};
use serde::Serialize;
use web_transport_quinn::SendStream;

use super::{
    Envelope, ProtocolHandler,
    envelope_io::{bad_request, event_envelope, payload_as, response_envelope, write_envelope},
    logs_forwarder::LogsForwarder,
    session_loop::ProtocolSession,
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
            MessageType::CommandCancel => self.handle_command_cancel(request).await,
            _ => Err(ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "dispatch",
                "unsupported message type",
            )),
        }
    }

    fn handle_hello(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: imago_protocol::HelloNegotiateRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("hello.negotiate", e.to_string()))?;

        let accepted =
            is_compatible_date_match(&payload.compatibility_date, &self.config.compatibility_date);
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

        response_envelope(
            MessageType::HelloNegotiate,
            request.request_id,
            request.correlation_id,
            &imago_protocol::HelloNegotiateResponse {
                accepted,
                server_version: self.config.server_version.clone(),
                features: vec![
                    "hello.negotiate".to_string(),
                    "deploy.prepare".to_string(),
                    "artifact.push".to_string(),
                    "artifact.commit".to_string(),
                    "command.start".to_string(),
                    "command.event".to_string(),
                    "state.request".to_string(),
                    "command.cancel".to_string(),
                    "logs.request".to_string(),
                    "logs.chunk".to_string(),
                    "logs.end".to_string(),
                ],
                limits,
            },
        )
    }

    async fn handle_prepare(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: DeployPrepareRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("deploy.prepare", e.to_string()))?;

        let response = self.artifacts.prepare(payload).await?;
        response_envelope(
            MessageType::DeployPrepare,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_push(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: ArtifactPushRequest = payload_as(&request)?;
        validate_push_payload(&payload)?;

        let response = self.artifacts.push(payload).await?;
        response_envelope(
            MessageType::ArtifactPush,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_commit(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: imago_protocol::ArtifactCommitRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("artifact.commit", e.to_string()))?;

        let response = self.artifacts.commit(payload).await?;
        response_envelope(
            MessageType::ArtifactCommit,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_state_request(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: StateRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("state.request", e.to_string()))?;

        let response = self
            .operations
            .snapshot_running(&payload.request_id)
            .await?;
        response_envelope(
            MessageType::StateResponse,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_command_cancel(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: CommandCancelRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("command.cancel", e.to_string()))?;

        let response = self.operations.request_cancel(&payload.request_id).await?;
        response_envelope(
            MessageType::CommandCancel,
            request.request_id,
            request.correlation_id,
            &response,
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
        let payload: imago_protocol::LogRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("logs.request", e.to_string()))?;

        let service_names = match payload.name {
            Some(name) => vec![name],
            None => {
                let names = self.orchestrator.running_service_names().await;
                if names.is_empty() {
                    return Err(ImagodError::new(
                        imago_protocol::ErrorCode::NotFound,
                        "logs.request",
                        "no running services are available",
                    ));
                }
                names
            }
        };

        let mut subscriptions = Vec::with_capacity(service_names.len());
        for service_name in &service_names {
            let subscription = self
                .orchestrator
                .open_logs(service_name, payload.tail_lines, payload.follow)
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
            request.request_id,
            request.correlation_id,
            &LogsRequestAck {
                accepted: true,
                names: service_names,
                follow: payload.follow,
            },
        )?;
        write_envelope(send, &ack, self.frame_codec.as_ref()).await?;

        let logs_forwarder = self.logs_forwarder.clone();
        tokio::spawn(async move {
            logs_forwarder
                .forward(
                    session,
                    request.request_id,
                    request.correlation_id,
                    subscriptions,
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
        let payload: CommandStartRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("command.start", e.to_string()))?;

        ensure_command_start_request_id_match(request.request_id, payload.request_id)?;
        ensure_command_start_allowed(&self.shutdown_requested)?;
        let operation_id = request.request_id;

        self.operations
            .start(operation_id, payload.command_type)
            .await?;

        let accepted = response_envelope(
            MessageType::CommandStart,
            request.request_id,
            request.correlation_id,
            &CommandStartResponse { accepted: true },
        )?;
        write_envelope(send, &accepted, self.frame_codec.as_ref()).await?;

        let accepted_event = event_envelope(
            self.clock.as_ref(),
            operation_id,
            request.correlation_id,
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
            request.correlation_id,
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
                request.correlation_id,
                CommandEventType::Canceled,
                payload.command_type,
                Some("canceled".to_string()),
                None,
            )?;
            let canceled_write = write_envelope(send, &canceled, self.frame_codec.as_ref()).await;
            finalize_operation_after_terminal_event(
                &self.operations,
                &operation_id,
                CommandState::Canceled,
                "canceled",
                canceled_write,
            )
            .await?;
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
                    request.correlation_id,
                    CommandEventType::Progress,
                    payload.command_type,
                    Some(progress_stage),
                    None,
                )?;
                write_envelope(send, &progress, self.frame_codec.as_ref()).await?;

                let succeeded = event_envelope(
                    self.clock.as_ref(),
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Succeeded,
                    payload.command_type,
                    Some(success_stage_for_event),
                    None,
                )?;
                let succeeded_write =
                    write_envelope(send, &succeeded, self.frame_codec.as_ref()).await;
                finalize_operation_after_terminal_event(
                    &self.operations,
                    &operation_id,
                    CommandState::Succeeded,
                    success_stage,
                    succeeded_write,
                )
                .await?;
            }
            Err(err) => {
                let failed = event_envelope(
                    self.clock.as_ref(),
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Failed,
                    payload.command_type,
                    Some("failed".to_string()),
                    Some(err.to_structured()),
                )?;
                let failed_write = write_envelope(send, &failed, self.frame_codec.as_ref()).await;
                finalize_operation_after_terminal_event(
                    &self.operations,
                    &operation_id,
                    CommandState::Failed,
                    "failed",
                    failed_write,
                )
                .await?;
            }
        }

        Ok(())
    }
}

pub(crate) fn is_compatible_date_match(request: &str, configured: &str) -> bool {
    request == configured
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
