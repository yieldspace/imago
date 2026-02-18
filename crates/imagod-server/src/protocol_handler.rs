//! Deploy protocol session handler and message dispatch implementation.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use imagod_common::ImagodError;
use imagod_config::ImagodConfig;
use imagod_control::{ArtifactStore, OperationManager, Orchestrator};
use serde_json::Value;
use web_transport_quinn::Session;

mod clock;
mod codec;
mod envelope_io;
mod logs_forwarder;
mod router;
mod session_loop;

pub(crate) const MAX_STREAM_BYTES: usize = 1024 * 1024 * 16;
pub(crate) const STREAM_READ_TIMEOUT_SECS: u64 = 30;
pub(crate) const LOG_DATAGRAM_TARGET_BYTES: usize = 1024;

/// JSON-backed envelope type used by stream decode/encode flow.
pub(crate) type Envelope = imago_protocol::ProtocolEnvelope<Value>;

#[derive(Clone)]
/// Handles one WebTransport session and dispatches protocol messages.
pub struct ProtocolHandler {
    config: Arc<ImagodConfig>,
    artifacts: ArtifactStore,
    operations: OperationManager,
    orchestrator: Orchestrator,
    shutdown_requested: Arc<AtomicBool>,
    frame_codec: Arc<codec::LengthPrefixedFrameCodec>,
    clock: Arc<clock::SystemServerClock>,
    logs_forwarder: Arc<logs_forwarder::DefaultLogsForwarder>,
}

impl ProtocolHandler {
    /// Creates a protocol handler with shared manager dependencies.
    pub fn new(
        config: Arc<ImagodConfig>,
        artifacts: ArtifactStore,
        operations: OperationManager,
        orchestrator: Orchestrator,
    ) -> Self {
        Self::with_runtime_components(
            config,
            artifacts,
            operations,
            orchestrator,
            Arc::new(codec::LengthPrefixedFrameCodec),
            Arc::new(clock::SystemServerClock),
            Arc::new(logs_forwarder::DefaultLogsForwarder),
        )
    }

    fn with_runtime_components(
        config: Arc<ImagodConfig>,
        artifacts: ArtifactStore,
        operations: OperationManager,
        orchestrator: Orchestrator,
        frame_codec: Arc<codec::LengthPrefixedFrameCodec>,
        clock: Arc<clock::SystemServerClock>,
        logs_forwarder: Arc<logs_forwarder::DefaultLogsForwarder>,
    ) -> Self {
        Self {
            config,
            artifacts,
            operations,
            orchestrator,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            frame_codec,
            clock,
            logs_forwarder,
        }
    }

    /// Rejects new start commands and transitions handler into shutdown mode.
    pub fn begin_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
    }

    /// Serves one WebTransport session until peer closes it.
    pub async fn handle_session(&self, session: Session) -> Result<(), ImagodError> {
        let session = Arc::new(session);
        session_loop::run_session_loop(self, session).await
    }

    /// Reaps finished services via orchestrator.
    pub async fn reap_finished_services(&self) {
        self.orchestrator.reap_finished_services().await;
    }

    /// Returns whether any managed services are currently alive.
    pub async fn has_live_services(&self) -> bool {
        self.orchestrator.has_live_services().await
    }

    /// Stops all managed services.
    pub async fn stop_all_services(&self, force: bool) -> Vec<(String, ImagodError)> {
        self.orchestrator.stop_all_services(force).await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        envelope_io::{
            ensure_non_nil_envelope_ids, ensure_single_request_envelope,
            response_message_type_for_request,
        },
        logs_forwarder::{
            advance_seq_for_lagged, fixed_log_chunk_size, log_error_from_imagod_error,
            service_log_stream_to_protocol,
        },
        router::{
            ensure_command_start_allowed, ensure_command_start_request_id_match,
            finalize_operation_after_terminal_event, is_compatible_date_match,
            validate_push_payload,
        },
        session_loop::{read_stream_with_timeout, stream_read_timeout_error},
    };
    use imago_protocol::{
        ArtifactPushChunkHeader, ArtifactPushRequest, CommandState, CommandType, ErrorCode,
        LogChunk, LogErrorCode, LogStreamKind, MessageType, ProtocolEnvelope, to_cbor,
    };
    use imagod_common::ImagodError;
    use imagod_control::{OperationManager, ServiceLogStream};
    use serde_json::Value;
    use std::{sync::atomic::AtomicBool, time::Duration};
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
    fn state_request_errors_use_state_response_message_type() {
        assert_eq!(
            response_message_type_for_request(MessageType::StateRequest),
            MessageType::StateResponse
        );
    }

    #[test]
    fn non_state_request_errors_keep_original_message_type() {
        assert_eq!(
            response_message_type_for_request(MessageType::DeployPrepare),
            MessageType::DeployPrepare
        );
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

    #[test]
    fn command_start_is_rejected_when_shutdown_requested() {
        let shutdown_requested = AtomicBool::new(true);
        let err = ensure_command_start_allowed(&shutdown_requested)
            .expect_err("shutdown mode should reject command.start");
        assert_eq!(err.code, imago_protocol::ErrorCode::Busy);
        assert_eq!(err.stage, "command.start");
        assert_eq!(err.message, "server is shutting down");
    }

    #[test]
    fn rejects_nil_request_id_in_envelope() {
        let envelope = ProtocolEnvelope {
            message_type: MessageType::HelloNegotiate,
            request_id: Uuid::nil(),
            correlation_id: Uuid::new_v4(),
            payload: Value::Null,
            error: None,
        };
        assert!(ensure_non_nil_envelope_ids(&envelope).is_err());
    }

    #[test]
    fn rejects_nil_correlation_id_in_envelope() {
        let envelope = ProtocolEnvelope {
            message_type: MessageType::HelloNegotiate,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::nil(),
            payload: Value::Null,
            error: None,
        };
        assert!(ensure_non_nil_envelope_ids(&envelope).is_err());
    }

    #[test]
    fn rejects_empty_chunk_b64_in_handle_push_validation() {
        let payload = ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: "deploy-1".to_string(),
                offset: 0,
                length: 4,
                chunk_sha256: "abcd".to_string(),
                upload_token: "token-1".to_string(),
            },
            chunk_b64: String::new(),
        };
        let err = validate_push_payload(&payload).expect_err("empty chunk_b64 should be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
    }

    #[tokio::test]
    async fn read_stream_timeout_returns_operation_timeout() {
        let err = read_stream_with_timeout(
            std::future::pending::<Result<Vec<u8>, std::io::Error>>(),
            Duration::from_millis(1),
        )
        .await
        .expect_err("pending read should timeout");
        assert_eq!(err.code, imago_protocol::ErrorCode::OperationTimeout);
    }

    #[test]
    fn stream_timeout_error_has_session_read_stage() {
        let err = stream_read_timeout_error();
        assert_eq!(err.code, imago_protocol::ErrorCode::OperationTimeout);
        assert_eq!(err.stage, "session.read");
    }

    #[tokio::test]
    async fn finalize_terminal_operation_removes_operation_even_when_stream_write_failed() {
        let operations = OperationManager::new();
        let request_id = Uuid::new_v4();
        operations
            .start(request_id, CommandType::Deploy)
            .await
            .expect("start should succeed");
        operations
            .set_state(&request_id, CommandState::Running, "running")
            .await
            .expect("state update should succeed");

        let write_error = ImagodError::new(ErrorCode::Internal, "session.write", "stream closed");
        let result = finalize_operation_after_terminal_event(
            &operations,
            &request_id,
            CommandState::Failed,
            "failed",
            Err(write_error),
        )
        .await;

        assert!(result.is_err());
        let snapshot = operations.snapshot_running(&request_id).await;
        assert!(
            snapshot.is_err(),
            "operation should be removed after finalize"
        );
    }

    #[test]
    fn fixed_log_chunk_size_is_bounded_by_target_and_datagram_size() {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let max_datagram_size = 1413usize;
        let chunk_size = fixed_log_chunk_size(
            request_id,
            correlation_id,
            max_datagram_size,
            &["svc-a".to_string()],
        )
        .expect("chunk size should be computed");
        assert!(chunk_size <= 1024);
        assert!(chunk_size > 0);

        let probe = ProtocolEnvelope::new(
            MessageType::LogsChunk,
            request_id,
            correlation_id,
            LogChunk {
                request_id,
                seq: u64::MAX,
                name: "svc-a".to_string(),
                stream_kind: LogStreamKind::Composite,
                bytes: vec![0xAB; chunk_size],
                is_last: false,
            },
        );
        let encoded = to_cbor(&probe).expect("encoding must succeed");
        assert!(encoded.len() <= max_datagram_size);
    }

    #[test]
    fn fixed_log_chunk_size_handles_long_name() {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let name = "service-with-very-long-name-".repeat(12);
        let max_datagram_size = 1413usize;
        let chunk_size = fixed_log_chunk_size(
            request_id,
            correlation_id,
            max_datagram_size,
            std::slice::from_ref(&name),
        )
        .expect("chunk size should be computed");
        assert!(chunk_size > 0);

        let probe = ProtocolEnvelope::new(
            MessageType::LogsChunk,
            request_id,
            correlation_id,
            LogChunk {
                request_id,
                seq: u64::MAX,
                name,
                stream_kind: LogStreamKind::Composite,
                bytes: vec![0xCD; chunk_size],
                is_last: false,
            },
        );
        let encoded = to_cbor(&probe).expect("encoding must succeed");
        assert!(encoded.len() <= max_datagram_size);
    }

    #[test]
    fn fixed_log_chunk_size_rejects_too_small_datagram() {
        let err = fixed_log_chunk_size(Uuid::new_v4(), Uuid::new_v4(), 8, &["svc-a".to_string()])
            .expect_err("small datagram should be rejected");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "logs.datagram");
    }

    #[test]
    fn log_error_mapping_preserves_common_error_kinds() {
        let not_found = ImagodError::new(ErrorCode::NotFound, "logs", "missing service");
        let mapped_not_found = log_error_from_imagod_error(&not_found);
        assert_eq!(mapped_not_found.code, LogErrorCode::ProcessNotFound);

        let unauthorized = ImagodError::new(ErrorCode::Unauthorized, "logs", "denied");
        let mapped_unauthorized = log_error_from_imagod_error(&unauthorized);
        assert_eq!(mapped_unauthorized.code, LogErrorCode::PermissionDenied);
    }

    #[test]
    fn service_log_stream_maps_to_protocol_stream_kind() {
        assert_eq!(
            service_log_stream_to_protocol(ServiceLogStream::Stdout),
            LogStreamKind::Stdout
        );
        assert_eq!(
            service_log_stream_to_protocol(ServiceLogStream::Stderr),
            LogStreamKind::Stderr
        );
    }

    #[test]
    fn advance_seq_for_lagged_increments_seq_by_dropped_count() {
        let mut seq = 42u64;
        advance_seq_for_lagged(&mut seq, 7);
        assert_eq!(seq, 49);
    }

    #[test]
    fn advance_seq_for_lagged_saturates_on_overflow() {
        let mut seq = u64::MAX - 1;
        advance_seq_for_lagged(&mut seq, 3);
        assert_eq!(seq, u64::MAX);
    }
}
