//! Deploy protocol session handler and message dispatch implementation.
//!
//! This layer owns envelope routing, request stream limits, and role-aware
//! dynamic key checks before delegating to orchestration/control components.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_config::{ImagodConfig, parse_ed25519_raw_public_key_hex, resolve_config_path};
use imagod_control::{ArtifactStore, OperationManager, Orchestrator};
use serde_json::Value;
use web_transport_quinn::Session;

#[cfg(feature = "bench-internals")]
pub mod bench_internals;
mod clock;
mod codec;
mod envelope_io;
mod logs_forwarder;
mod router;
mod session_loop;

pub(crate) const MAX_STREAM_BYTES: usize = 1024 * 1024 * 16;
pub(crate) const STREAM_READ_TIMEOUT_SECS: u64 = 30;
pub(crate) const LOG_DATAGRAM_TARGET_BYTES: usize = 1024;

const STAGE_DYNAMIC_KEYS: &str = "protocol.keys";

/// JSON-backed envelope type used by stream decode/encode flow.
pub(crate) type Envelope = imago_protocol::ProtocolEnvelope<Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DynamicClientRole {
    Admin,
    Client,
    Unknown,
}

#[derive(Debug, Default)]
struct DynamicPublicKeys {
    admin_keys: HashSet<[u8; 32]>,
    client_keys: HashSet<[u8; 32]>,
}

static DYNAMIC_PUBLIC_KEYS: OnceLock<RwLock<DynamicPublicKeys>> = OnceLock::new();
#[cfg(test)]
static DYNAMIC_PUBLIC_KEYS_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
thread_local! {
    static DYNAMIC_PUBLIC_KEYS_TEST_LOCK_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn dynamic_public_keys() -> &'static RwLock<DynamicPublicKeys> {
    DYNAMIC_PUBLIC_KEYS.get_or_init(|| RwLock::new(DynamicPublicKeys::default()))
}

#[cfg(test)]
pub(crate) struct DynamicPublicKeysTestGuard {
    _lock: Option<std::sync::MutexGuard<'static, ()>>,
}

#[cfg(test)]
impl Drop for DynamicPublicKeysTestGuard {
    fn drop(&mut self) {
        DYNAMIC_PUBLIC_KEYS_TEST_LOCK_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current.saturating_sub(1));
        });
    }
}

#[cfg(test)]
pub(crate) fn lock_dynamic_public_keys_for_tests() -> DynamicPublicKeysTestGuard {
    let is_outermost = DYNAMIC_PUBLIC_KEYS_TEST_LOCK_DEPTH.with(|depth| {
        let current = depth.get();
        depth.set(current + 1);
        current == 0
    });
    if is_outermost {
        let lock = match DYNAMIC_PUBLIC_KEYS_TEST_MUTEX.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        return DynamicPublicKeysTestGuard { _lock: Some(lock) };
    }
    DynamicPublicKeysTestGuard { _lock: None }
}

fn parse_configured_public_keys(
    keys: &[String],
    field_name: &'static str,
) -> Result<HashSet<[u8; 32]>, ImagodError> {
    let mut parsed = HashSet::with_capacity(keys.len());
    for (index, key_hex) in keys.iter().enumerate() {
        let key = parse_ed25519_raw_public_key_hex(key_hex).map_err(|reason| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_DYNAMIC_KEYS,
                format!("invalid {field_name}[{index}]: {reason}"),
            )
        })?;
        parsed.insert(key);
    }
    Ok(parsed)
}

pub(crate) fn sync_dynamic_public_keys_from_config(
    config: &ImagodConfig,
) -> Result<(), ImagodError> {
    #[cfg(test)]
    let _test_guard = lock_dynamic_public_keys_for_tests();

    let updated = DynamicPublicKeys {
        admin_keys: parse_configured_public_keys(
            &config.tls.admin_public_keys,
            "tls.admin_public_keys",
        )?,
        client_keys: parse_configured_public_keys(
            &config.tls.client_public_keys,
            "tls.client_public_keys",
        )?,
    };
    let mut guard = match dynamic_public_keys().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = updated;
    Ok(())
}

pub(crate) fn upsert_dynamic_client_public_key(public_key_hex: &str) -> Result<bool, ImagodError> {
    #[cfg(test)]
    let _test_guard = lock_dynamic_public_keys_for_tests();

    let key = parse_ed25519_raw_public_key_hex(public_key_hex).map_err(|reason| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "bindings.cert.upload",
            format!("public_key_hex is invalid: {reason}"),
        )
    })?;
    let mut guard = match dynamic_public_keys().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    Ok(guard.client_keys.insert(key))
}

pub(crate) fn resolve_dynamic_client_role(public_key: &[u8; 32]) -> DynamicClientRole {
    #[cfg(test)]
    let _test_guard = lock_dynamic_public_keys_for_tests();

    let guard = match dynamic_public_keys().read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.admin_keys.contains(public_key) {
        return DynamicClientRole::Admin;
    }
    if guard.client_keys.contains(public_key) {
        return DynamicClientRole::Client;
    }
    DynamicClientRole::Unknown
}

pub(crate) fn is_tls_client_key_allowlisted(public_key: &[u8; 32]) -> bool {
    #[cfg(test)]
    let _test_guard = lock_dynamic_public_keys_for_tests();

    let guard = match dynamic_public_keys().read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.admin_keys.contains(public_key) || guard.client_keys.contains(public_key)
}

#[cfg(test)]
pub(crate) fn replace_dynamic_public_keys_for_tests(
    admin_keys: &[[u8; 32]],
    client_keys: &[[u8; 32]],
) {
    let _test_guard = lock_dynamic_public_keys_for_tests();

    let mut guard = match dynamic_public_keys().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.admin_keys = admin_keys.iter().copied().collect();
    guard.client_keys = client_keys.iter().copied().collect();
}

#[derive(Clone)]
/// Handles one WebTransport session and dispatches protocol messages.
pub struct ProtocolHandler {
    config: Arc<ImagodConfig>,
    config_path: PathBuf,
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
        config_path: PathBuf,
        artifacts: ArtifactStore,
        operations: OperationManager,
        orchestrator: Orchestrator,
    ) -> Self {
        Self::new_with_runtime_components(
            config,
            config_path,
            artifacts,
            operations,
            orchestrator,
            Arc::new(codec::LengthPrefixedFrameCodec),
            Arc::new(clock::SystemServerClock),
            Arc::new(logs_forwarder::DefaultLogsForwarder),
        )
    }

    /// Creates a protocol handler with default config path resolution.
    pub fn new_with_default_config_path(
        config: Arc<ImagodConfig>,
        artifacts: ArtifactStore,
        operations: OperationManager,
        orchestrator: Orchestrator,
    ) -> Self {
        Self::new(
            config,
            resolve_config_path(None),
            artifacts,
            operations,
            orchestrator,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_runtime_components(
        config: Arc<ImagodConfig>,
        config_path: PathBuf,
        artifacts: ArtifactStore,
        operations: OperationManager,
        orchestrator: Orchestrator,
        frame_codec: Arc<codec::LengthPrefixedFrameCodec>,
        clock: Arc<clock::SystemServerClock>,
        logs_forwarder: Arc<logs_forwarder::DefaultLogsForwarder>,
    ) -> Self {
        sync_dynamic_public_keys_from_config(config.as_ref())
            .expect("validated config should contain valid TLS public keys");
        Self {
            config,
            config_path,
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
            finalize_operation_after_terminal_event, protocol_compatibility_announcement,
            validate_push_payload,
        },
        session_loop::{read_stream_with_timeout, stream_read_timeout_error},
    };
    use imago_protocol::{
        ArtifactPushChunkHeader, ArtifactPushRequest, CommandErrorKind, CommandKind,
        CommandProtocolAction, CommandProtocolContext, CommandProtocolOutput,
        CommandProtocolStageId, ErrorCode, LogChunk, LogErrorCode, LogStreamKind, MessageType,
        ProtocolEnvelope, to_cbor,
    };
    use imagod_common::ImagodError;
    use imagod_control::{ActionApplier, OperationManager, ServiceLogStream};
    use serde_json::Value;
    use std::{sync::atomic::AtomicBool, time::Duration};
    use uuid::Uuid;

    #[test]
    fn accepts_supported_protocol_version() {
        assert!(protocol_compatibility_announcement("0.1.0").is_none());
    }

    #[test]
    fn rejects_unsupported_protocol_version() {
        let message = protocol_compatibility_announcement("0.2.0")
            .expect("unsupported version should return announcement");
        assert!(message.contains("not supported"));
    }

    #[test]
    fn rejects_invalid_protocol_version() {
        let message = protocol_compatibility_announcement("not-semver")
            .expect("invalid semver should return announcement");
        assert!(message.contains("invalid"));
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
    fn rejects_empty_chunk_in_handle_push_validation() {
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
        let err = validate_push_payload(&payload).expect_err("empty chunk should be rejected");
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

        let write_error = ImagodError::new(ErrorCode::Internal, "session.write", "stream closed");
        let result = finalize_operation_after_terminal_event(
            &operations,
            &CommandProtocolContext { request_id },
            CommandProtocolAction::FinishFailed(CommandErrorKind::Internal),
            Err(write_error),
        )
        .await;

        assert!(result.is_err());
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
            false,
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
                timestamp_unix_ms: None,
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
            false,
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
                timestamp_unix_ms: None,
            },
        );
        let encoded = to_cbor(&probe).expect("encoding must succeed");
        assert!(encoded.len() <= max_datagram_size);
    }

    #[test]
    fn fixed_log_chunk_size_rejects_too_small_datagram() {
        let err = fixed_log_chunk_size(
            Uuid::new_v4(),
            Uuid::new_v4(),
            8,
            &["svc-a".to_string()],
            false,
        )
        .expect_err("small datagram should be rejected");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "logs.datagram");
    }

    #[test]
    fn fixed_log_chunk_size_accounts_for_timestamp_overhead() {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let max_datagram_size = 1413usize;
        let chunk_size = fixed_log_chunk_size(
            request_id,
            correlation_id,
            max_datagram_size,
            &["svc-a".to_string()],
            true,
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
                bytes: vec![0xEF; chunk_size],
                is_last: false,
                timestamp_unix_ms: Some(u64::MAX),
            },
        );
        let encoded = to_cbor(&probe).expect("encoding must succeed");
        assert!(encoded.len() <= max_datagram_size);
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
