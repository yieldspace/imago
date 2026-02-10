use std::{collections::BTreeMap, sync::Arc, time::UNIX_EPOCH};

use imago_protocol::{
    CommandCancelRequest, CommandEvent, CommandStartRequest, CommandStartResponse, CommandType,
    DeployCommandPayload, DeployPrepareRequest, Envelope, ErrorCode, EventType,
    HelloNegotiateRequest, HelloNegotiateResponse, MESSAGE_ARTIFACT_COMMIT, MESSAGE_ARTIFACT_PUSH,
    MESSAGE_COMMAND_CANCEL, MESSAGE_COMMAND_EVENT, MESSAGE_COMMAND_START, MESSAGE_DEPLOY_PREPARE,
    MESSAGE_HELLO_NEGOTIATE, MESSAGE_STATE_REQUEST, OperationState, ProtocolError,
    RunCommandPayload, StateRequest, StopCommandPayload, decode_frames, encode_frame, from_cbor,
    to_cbor,
};
use web_transport_quinn::{SendStream, Session};

use crate::{
    artifact_store::ArtifactStore, config::ImagodConfig, error::ImagodError,
    operation_state::OperationManager, orchestrator::Orchestrator,
};

const MAX_STREAM_BYTES: usize = 1024 * 1024 * 16;

#[derive(Clone)]
pub struct ProtocolHandler {
    config: Arc<ImagodConfig>,
    artifacts: ArtifactStore,
    operations: OperationManager,
    orchestrator: Orchestrator,
}

impl ProtocolHandler {
    pub fn new(
        config: Arc<ImagodConfig>,
        artifacts: ArtifactStore,
        operations: OperationManager,
        orchestrator: Orchestrator,
    ) -> Self {
        Self {
            config,
            artifacts,
            operations,
            orchestrator,
        }
    }

    pub async fn handle_session(&self, session: Session) -> Result<(), ImagodError> {
        loop {
            let (mut send, mut recv) = match session.accept_bi().await {
                Ok(streams) => streams,
                Err(_) => break,
            };

            let buf = recv.read_to_end(MAX_STREAM_BYTES).await.map_err(|e| {
                ImagodError::new(
                    ErrorCode::BadRequest,
                    "session.read",
                    format!("failed to read stream: {e}"),
                )
            })?;

            let envelopes = match parse_stream_envelopes(&buf) {
                Ok(v) => v,
                Err(err) => {
                    let envelope =
                        Envelope::error("unknown", "unknown", "unknown", protocol_error(err));
                    write_envelope(&mut send, &envelope).await?;
                    send.finish().map_err(|e| {
                        ImagodError::new(
                            ErrorCode::Internal,
                            "session.write",
                            format!("failed to finish stream: {e}"),
                        )
                    })?;
                    continue;
                }
            };

            if envelopes.is_empty() {
                send.finish().map_err(|e| {
                    ImagodError::new(
                        ErrorCode::Internal,
                        "session.write",
                        format!("failed to finish stream: {e}"),
                    )
                })?;
                continue;
            }

            let request = envelopes[0].clone();
            if request.message_type == MESSAGE_COMMAND_START {
                self.handle_command_start(request, &mut send).await?;
                send.finish().map_err(|e| {
                    ImagodError::new(
                        ErrorCode::Internal,
                        "session.write",
                        format!("failed to finish stream: {e}"),
                    )
                })?;
                continue;
            }

            let response = match self.handle_single(request.clone()).await {
                Ok(resp) => resp,
                Err(err) => Envelope::error(
                    request.message_type,
                    request.request_id,
                    request.correlation_id,
                    err.to_structured(),
                ),
            };
            write_envelope(&mut send, &response).await?;
            send.finish().map_err(|e| {
                ImagodError::new(
                    ErrorCode::Internal,
                    "session.write",
                    format!("failed to finish stream: {e}"),
                )
            })?;
        }

        Ok(())
    }

    pub async fn reap_finished_services(&self) {
        self.orchestrator.reap_finished_services().await;
    }

    pub async fn has_live_services(&self) -> bool {
        self.orchestrator.has_live_services().await
    }

    async fn handle_single(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        match request.message_type.as_str() {
            MESSAGE_HELLO_NEGOTIATE => self.handle_hello(request),
            MESSAGE_DEPLOY_PREPARE => self.handle_prepare(request).await,
            MESSAGE_ARTIFACT_PUSH => self.handle_push(request).await,
            MESSAGE_ARTIFACT_COMMIT => self.handle_commit(request).await,
            MESSAGE_STATE_REQUEST => self.handle_state_request(request).await,
            MESSAGE_COMMAND_CANCEL => self.handle_command_cancel(request).await,
            _ => Err(ImagodError::new(
                ErrorCode::BadRequest,
                "dispatch",
                "unsupported message type",
            )),
        }
    }

    fn handle_hello(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: HelloNegotiateRequest = request.payload_as().map_err(protocol_bad_request)?;
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
            "upload_session_ttl".to_string(),
            format!("{}s", self.config.runtime.upload_session_ttl_secs),
        );

        Envelope::response(
            MESSAGE_HELLO_NEGOTIATE,
            request.request_id,
            request.correlation_id,
            &HelloNegotiateResponse {
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
                ],
                limits,
            },
        )
        .map_err(protocol_bad_request)
    }

    async fn handle_prepare(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: DeployPrepareRequest = request.payload_as().map_err(protocol_bad_request)?;
        let response = self.artifacts.prepare(payload).await?;
        Envelope::response(
            MESSAGE_DEPLOY_PREPARE,
            request.request_id,
            request.correlation_id,
            &response,
        )
        .map_err(protocol_bad_request)
    }

    async fn handle_push(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload = request.payload_as().map_err(protocol_bad_request)?;
        let response = self.artifacts.push(payload).await?;
        Envelope::response(
            MESSAGE_ARTIFACT_PUSH,
            request.request_id,
            request.correlation_id,
            &response,
        )
        .map_err(protocol_bad_request)
    }

    async fn handle_commit(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload = request.payload_as().map_err(protocol_bad_request)?;
        let response = self.artifacts.commit(payload).await?;
        Envelope::response(
            MESSAGE_ARTIFACT_COMMIT,
            request.request_id,
            request.correlation_id,
            &response,
        )
        .map_err(protocol_bad_request)
    }

    async fn handle_state_request(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: StateRequest = request.payload_as().map_err(protocol_bad_request)?;
        let response = self
            .operations
            .snapshot_running(&payload.request_id)
            .await?;
        Envelope::response(
            MESSAGE_STATE_REQUEST,
            request.request_id,
            request.correlation_id,
            &response,
        )
        .map_err(protocol_bad_request)
    }

    async fn handle_command_cancel(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: CommandCancelRequest = request.payload_as().map_err(protocol_bad_request)?;
        let response = self.operations.request_cancel(&payload.request_id).await?;
        Envelope::response(
            MESSAGE_COMMAND_CANCEL,
            request.request_id,
            request.correlation_id,
            &response,
        )
        .map_err(protocol_bad_request)
    }

    async fn handle_command_start(
        &self,
        request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError> {
        let payload: CommandStartRequest = request.payload_as().map_err(protocol_bad_request)?;
        let command_type = payload.command_type;

        self.operations
            .start(payload.request_id.clone(), command_type)
            .await?;

        let accepted = Envelope::response(
            MESSAGE_COMMAND_START,
            request.request_id.clone(),
            request.correlation_id.clone(),
            &CommandStartResponse { accepted: true },
        )
        .map_err(protocol_bad_request)?;
        write_envelope(send, &accepted).await?;

        let accepted_event = event_envelope(
            &payload.request_id,
            &request.correlation_id,
            EventType::Accepted,
            command_type,
            None,
            None,
        )?;
        write_envelope(send, &accepted_event).await?;

        self.operations
            .set_state(&payload.request_id, OperationState::Running, "starting")
            .await?;
        let running_event = event_envelope(
            &payload.request_id,
            &request.correlation_id,
            EventType::Progress,
            command_type,
            Some("starting".to_string()),
            None,
        )?;
        write_envelope(send, &running_event).await?;

        if self
            .operations
            .is_cancel_requested(&payload.request_id)
            .await
        {
            self.operations
                .finish(&payload.request_id, OperationState::Canceled, "canceled")
                .await;
            let canceled = event_envelope(
                &payload.request_id,
                &request.correlation_id,
                EventType::Canceled,
                command_type,
                Some("canceled".to_string()),
                None,
            )?;
            write_envelope(send, &canceled).await?;
            self.operations.remove(&payload.request_id).await;
            return Ok(());
        }

        let command_result = match command_type {
            CommandType::Deploy => {
                let deploy_payload: DeployCommandPayload =
                    serde_json::from_value(payload.payload.clone()).map_err(|e| {
                        ImagodError::new(
                            ErrorCode::BadRequest,
                            MESSAGE_COMMAND_START,
                            format!("invalid deploy payload: {e}"),
                        )
                    })?;
                self.orchestrator
                    .deploy(&deploy_payload)
                    .await
                    .map(|summary| {
                        (
                            format!("release:{}:{}", summary.service_name, summary.release_hash),
                            "spawned".to_string(),
                        )
                    })
            }
            CommandType::Run => {
                let run_payload: RunCommandPayload =
                    serde_json::from_value(payload.payload.clone()).map_err(|e| {
                        ImagodError::new(
                            ErrorCode::BadRequest,
                            MESSAGE_COMMAND_START,
                            format!("invalid run payload: {e}"),
                        )
                    })?;
                self.orchestrator.run(&run_payload).await.map(|summary| {
                    (
                        format!("running:{}:{}", summary.service_name, summary.release_hash),
                        "spawned".to_string(),
                    )
                })
            }
            CommandType::Stop => {
                let stop_payload: StopCommandPayload =
                    serde_json::from_value(payload.payload.clone()).map_err(|e| {
                        ImagodError::new(
                            ErrorCode::BadRequest,
                            MESSAGE_COMMAND_START,
                            format!("invalid stop payload: {e}"),
                        )
                    })?;
                self.orchestrator.stop(&stop_payload).await.map(|summary| {
                    (
                        format!("stopped:{}", summary.service_name),
                        "completed".to_string(),
                    )
                })
            }
        };

        match command_result {
            Ok((progress_stage, success_stage)) => {
                self.operations
                    .mark_spawned(&payload.request_id, &success_stage)
                    .await?;
                self.operations
                    .finish(
                        &payload.request_id,
                        OperationState::Succeeded,
                        &success_stage,
                    )
                    .await;

                let progress = event_envelope(
                    &payload.request_id,
                    &request.correlation_id,
                    EventType::Progress,
                    command_type,
                    Some(progress_stage),
                    None,
                )?;
                write_envelope(send, &progress).await?;

                let succeeded = event_envelope(
                    &payload.request_id,
                    &request.correlation_id,
                    EventType::Succeeded,
                    command_type,
                    Some(success_stage),
                    None,
                )?;
                write_envelope(send, &succeeded).await?;
                self.operations.remove(&payload.request_id).await;
            }
            Err(err) => {
                self.operations
                    .finish(&payload.request_id, OperationState::Failed, "failed")
                    .await;

                let failed = event_envelope(
                    &payload.request_id,
                    &request.correlation_id,
                    EventType::Failed,
                    command_type,
                    Some("failed".to_string()),
                    Some(err.to_structured()),
                )?;
                write_envelope(send, &failed).await?;
                self.operations.remove(&payload.request_id).await;
            }
        }

        Ok(())
    }
}

fn parse_stream_envelopes(buf: &[u8]) -> Result<Vec<Envelope>, ProtocolError> {
    let frames = decode_frames(buf)?;
    frames
        .iter()
        .map(|frame| from_cbor::<Envelope>(frame))
        .collect()
}

async fn write_envelope(send: &mut SendStream, envelope: &Envelope) -> Result<(), ImagodError> {
    let data = to_cbor(envelope).map_err(protocol_bad_request)?;
    let framed = encode_frame(&data);
    send.write_all(&framed).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "session.write",
            format!("failed to send frame: {e}"),
        )
    })?;
    Ok(())
}

fn protocol_bad_request(err: ProtocolError) -> ImagodError {
    ImagodError::new(ErrorCode::BadRequest, "protocol", err.to_string())
}

fn protocol_error(err: ProtocolError) -> imago_protocol::StructuredError {
    ImagodError::new(ErrorCode::BadRequest, "protocol", err.to_string()).to_structured()
}

fn event_envelope(
    request_id: &str,
    correlation_id: &str,
    event_type: EventType,
    command_type: CommandType,
    stage: Option<String>,
    error: Option<imago_protocol::StructuredError>,
) -> Result<Envelope, ImagodError> {
    let payload = CommandEvent {
        event_type,
        request_id: request_id.to_string(),
        command_type,
        timestamp: now_unix_secs(),
        stage,
        error,
    };
    Envelope::response(
        MESSAGE_COMMAND_EVENT,
        request_id.to_string(),
        correlation_id.to_string(),
        &payload,
    )
    .map_err(protocol_bad_request)
}

fn now_unix_secs() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

fn is_compatible_date_match(request: &str, configured: &str) -> bool {
    request == configured
}

#[cfg(test)]
mod tests {
    use super::is_compatible_date_match;

    #[test]
    fn accepts_same_compatibility_date() {
        assert!(is_compatible_date_match("2026-02-10", "2026-02-10"));
    }

    #[test]
    fn rejects_different_compatibility_date() {
        assert!(!is_compatible_date_match("2026-02-11", "2026-02-10"));
    }
}
