use std::{collections::BTreeMap, sync::Arc, time::UNIX_EPOCH};

use imago_protocol::{
    ArtifactPushRequest, CommandCancelRequest, CommandEvent, CommandEventType, CommandPayload,
    CommandStartRequest, CommandStartResponse, CommandState, CommandType, DeployPrepareRequest,
    MessageType, ProtocolEnvelope, StateRequest, StructuredError, Validate, from_cbor, to_cbor,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;
use web_transport_quinn::{SendStream, Session};

use crate::{
    artifact_store::ArtifactStore, config::ImagodConfig, error::ImagodError,
    operation_state::OperationManager, orchestrator::Orchestrator,
};

const MAX_STREAM_BYTES: usize = 1024 * 1024 * 16;

type Envelope = ProtocolEnvelope<Value>;

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
                    imago_protocol::ErrorCode::BadRequest,
                    "session.read",
                    format!("failed to read stream: {e}"),
                )
            })?;

            let envelopes = match parse_stream_envelopes(&buf) {
                Ok(v) => v,
                Err(err) => {
                    let envelope = error_envelope(
                        MessageType::CommandEvent,
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        err.to_structured(),
                    );
                    write_envelope(&mut send, &envelope).await?;
                    finish_stream(&mut send)?;
                    continue;
                }
            };

            if envelopes.is_empty() {
                finish_stream(&mut send)?;
                continue;
            }

            if let Err(err) = ensure_single_request_envelope(&envelopes) {
                let first = &envelopes[0];
                let response = error_envelope(
                    first.message_type,
                    first.request_id,
                    first.correlation_id,
                    err.to_structured(),
                );
                write_envelope(&mut send, &response).await?;
                finish_stream(&mut send)?;
                continue;
            }

            let request = envelopes[0].clone();
            if request.message_type == MessageType::CommandStart {
                self.handle_command_start(request, &mut send).await?;
                finish_stream(&mut send)?;
                continue;
            }

            let response = match self.handle_single(request.clone()).await {
                Ok(resp) => resp,
                Err(err) => error_envelope(
                    request.message_type,
                    request.request_id,
                    request.correlation_id,
                    err.to_structured(),
                ),
            };
            write_envelope(&mut send, &response).await?;
            finish_stream(&mut send)?;
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
        payload
            .header
            .validate()
            .map_err(|e| bad_request("artifact.push", e.to_string()))?;

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

    async fn handle_command_start(
        &self,
        request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError> {
        let payload: CommandStartRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("command.start", e.to_string()))?;

        ensure_command_start_request_id_match(request.request_id, payload.request_id)?;
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
        write_envelope(send, &accepted).await?;

        let accepted_event = event_envelope(
            operation_id,
            request.correlation_id,
            CommandEventType::Accepted,
            payload.command_type,
            None,
            None,
        )?;
        write_envelope(send, &accepted_event).await?;

        self.operations
            .set_state(&operation_id, CommandState::Running, "starting")
            .await?;
        let running_event = event_envelope(
            operation_id,
            request.correlation_id,
            CommandEventType::Progress,
            payload.command_type,
            Some("starting".to_string()),
            None,
        )?;
        write_envelope(send, &running_event).await?;

        if self.operations.is_cancel_requested(&operation_id).await {
            self.operations
                .finish(&operation_id, CommandState::Canceled, "canceled")
                .await;
            let canceled = event_envelope(
                operation_id,
                request.correlation_id,
                CommandEventType::Canceled,
                payload.command_type,
                Some("canceled".to_string()),
                None,
            )?;
            write_envelope(send, &canceled).await?;
            self.operations.remove(&operation_id).await;
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
                self.operations
                    .mark_spawned(&operation_id, &success_stage)
                    .await?;
                self.operations
                    .finish(&operation_id, CommandState::Succeeded, &success_stage)
                    .await;

                let progress = event_envelope(
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Progress,
                    payload.command_type,
                    Some(progress_stage),
                    None,
                )?;
                write_envelope(send, &progress).await?;

                let succeeded = event_envelope(
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Succeeded,
                    payload.command_type,
                    Some(success_stage),
                    None,
                )?;
                write_envelope(send, &succeeded).await?;
                self.operations.remove(&operation_id).await;
            }
            Err(err) => {
                self.operations
                    .finish(&operation_id, CommandState::Failed, "failed")
                    .await;

                let failed = event_envelope(
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Failed,
                    payload.command_type,
                    Some("failed".to_string()),
                    Some(err.to_structured()),
                )?;
                write_envelope(send, &failed).await?;
                self.operations.remove(&operation_id).await;
            }
        }

        Ok(())
    }
}

fn response_envelope<T: Serialize>(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    payload: &T,
) -> Result<Envelope, ImagodError> {
    let payload = serde_json::to_value(payload)
        .map_err(|e| bad_request("protocol", format!("payload encode failed: {e}")))?;
    Ok(Envelope {
        message_type,
        request_id,
        correlation_id,
        payload,
        error: None,
    })
}

fn error_envelope(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    error: StructuredError,
) -> Envelope {
    Envelope {
        message_type,
        request_id,
        correlation_id,
        payload: Value::Null,
        error: Some(error),
    }
}

fn payload_as<T: DeserializeOwned>(request: &Envelope) -> Result<T, ImagodError> {
    serde_json::from_value(request.payload.clone())
        .map_err(|e| bad_request("protocol", format!("request payload decode failed: {e}")))
}

fn parse_stream_envelopes(buf: &[u8]) -> Result<Vec<Envelope>, ImagodError> {
    let frames = decode_frames(buf)?;
    frames
        .iter()
        .map(|frame| {
            from_cbor::<Envelope>(frame)
                .map_err(|e| bad_request("protocol", format!("invalid frame payload: {e}")))
        })
        .collect()
}

fn ensure_single_request_envelope(envelopes: &[Envelope]) -> Result<(), ImagodError> {
    if envelopes.len() > 1 {
        return Err(bad_request(
            "session.protocol",
            "multiple request envelopes on a single stream are not allowed",
        ));
    }
    Ok(())
}

async fn write_envelope(send: &mut SendStream, envelope: &Envelope) -> Result<(), ImagodError> {
    let data = to_cbor(envelope)
        .map_err(|e| bad_request("protocol", format!("cbor encode failed: {e}")))?;
    let framed = encode_frame(&data);
    send.write_all(&framed).await.map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "session.write",
            format!("failed to send frame: {e}"),
        )
    })?;
    Ok(())
}

fn finish_stream(send: &mut SendStream) -> Result<(), ImagodError> {
    send.finish().map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "session.write",
            format!("failed to finish stream: {e}"),
        )
    })
}

fn event_envelope(
    request_id: Uuid,
    correlation_id: Uuid,
    event_type: CommandEventType,
    command_type: CommandType,
    stage: Option<String>,
    error: Option<StructuredError>,
) -> Result<Envelope, ImagodError> {
    let payload = CommandEvent {
        event_type,
        request_id,
        command_type,
        timestamp: now_unix_secs(),
        stage,
        error,
    };
    response_envelope(
        MessageType::CommandEvent,
        request_id,
        correlation_id,
        &payload,
    )
}

fn now_unix_secs() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_frames(value: &[u8]) -> Result<Vec<Vec<u8>>, ImagodError> {
    let mut out = Vec::new();
    let mut offset = 0usize;

    while offset < value.len() {
        if value.len() - offset < 4 {
            return Err(bad_request("protocol", "truncated frame header"));
        }

        let len = u32::from_be_bytes(
            value[offset..offset + 4]
                .try_into()
                .map_err(|_| bad_request("protocol", "invalid frame header"))?,
        ) as usize;
        offset += 4;

        if value.len() - offset < len {
            return Err(bad_request("protocol", "truncated frame payload"));
        }

        out.push(value[offset..offset + len].to_vec());
        offset += len;
    }

    Ok(out)
}

fn bad_request(stage: &str, message: impl Into<String>) -> ImagodError {
    ImagodError::new(imago_protocol::ErrorCode::BadRequest, stage, message)
}

fn is_compatible_date_match(request: &str, configured: &str) -> bool {
    request == configured
}

fn ensure_command_start_request_id_match(
    envelope_request_id: Uuid,
    payload_request_id: Uuid,
) -> Result<(), ImagodError> {
    if envelope_request_id == payload_request_id {
        return Ok(());
    }

    Err(bad_request(
        "command.start",
        "envelope request_id and payload request_id must match",
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_command_start_request_id_match, ensure_single_request_envelope,
        is_compatible_date_match,
    };
    use imago_protocol::{MessageType, ProtocolEnvelope};
    use serde_json::Value;
    use uuid::Uuid;

    #[test]
    fn accepts_same_compatibility_date() {
        assert!(is_compatible_date_match("2026-02-10", "2026-02-10"));
    }

    #[test]
    fn rejects_different_compatibility_date() {
        assert!(!is_compatible_date_match("2026-02-11", "2026-02-10"));
    }

    #[test]
    fn rejects_multiple_request_envelopes_on_single_stream() {
        let envelope = ProtocolEnvelope {
            message_type: MessageType::HelloNegotiate,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: Value::Null,
            error: None,
        };
        let result = ensure_single_request_envelope(&[envelope.clone(), envelope]);
        assert!(result.is_err());
    }

    #[test]
    fn command_start_request_ids_must_match() {
        let envelope_request_id = Uuid::new_v4();
        let payload_request_id = Uuid::new_v4();
        let err = ensure_command_start_request_id_match(envelope_request_id, payload_request_id)
            .expect_err("mismatched request ids should be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
    }

    #[test]
    fn command_start_request_ids_can_match() {
        let request_id = Uuid::new_v4();
        let result = ensure_command_start_request_id_match(request_id, request_id);
        assert!(result.is_ok());
    }
}
