use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const MESSAGE_HELLO_NEGOTIATE: &str = "hello.negotiate";
pub const MESSAGE_DEPLOY_PREPARE: &str = "deploy.prepare";
pub const MESSAGE_ARTIFACT_PUSH: &str = "artifact.push";
pub const MESSAGE_ARTIFACT_COMMIT: &str = "artifact.commit";
pub const MESSAGE_COMMAND_START: &str = "command.start";
pub const MESSAGE_COMMAND_EVENT: &str = "command.event";
pub const MESSAGE_STATE_REQUEST: &str = "state.request";
pub const MESSAGE_COMMAND_CANCEL: &str = "command.cancel";

pub type JsonMap = BTreeMap<String, Value>;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("CBOR encode failed: {0}")]
    Encode(String),
    #[error("CBOR decode failed: {0}")]
    Decode(String),
    #[error("message payload is missing")]
    MissingPayload,
    #[error("message payload is invalid: {0}")]
    InvalidPayload(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub request_id: String,
    pub correlation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StructuredError>,
}

impl Envelope {
    pub fn request<T: Serialize>(
        message_type: impl Into<String>,
        request_id: impl Into<String>,
        correlation_id: impl Into<String>,
        payload: &T,
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            message_type: message_type.into(),
            request_id: request_id.into(),
            correlation_id: correlation_id.into(),
            payload: Some(
                serde_json::to_value(payload)
                    .map_err(|e| ProtocolError::InvalidPayload(e.to_string()))?,
            ),
            error: None,
        })
    }

    pub fn response<T: Serialize>(
        message_type: impl Into<String>,
        request_id: impl Into<String>,
        correlation_id: impl Into<String>,
        payload: &T,
    ) -> Result<Self, ProtocolError> {
        Self::request(message_type, request_id, correlation_id, payload)
    }

    pub fn error(
        message_type: impl Into<String>,
        request_id: impl Into<String>,
        correlation_id: impl Into<String>,
        error: StructuredError,
    ) -> Self {
        Self {
            message_type: message_type.into(),
            request_id: request_id.into(),
            correlation_id: correlation_id.into(),
            payload: None,
            error: Some(error),
        }
    }

    pub fn payload_as<T: DeserializeOwned>(&self) -> Result<T, ProtocolError> {
        let payload = self.payload.clone().ok_or(ProtocolError::MissingPayload)?;
        serde_json::from_value(payload).map_err(|e| ProtocolError::InvalidPayload(e.to_string()))
    }
}

pub fn to_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let mut buf = Vec::new();
    ciborium::ser::into_writer(value, &mut buf)
        .map_err(|e| ProtocolError::Encode(e.to_string()))?;
    Ok(buf)
}

pub fn from_cbor<T: DeserializeOwned>(value: &[u8]) -> Result<T, ProtocolError> {
    ciborium::de::from_reader(value).map_err(|e| ProtocolError::Decode(e.to_string()))
}

pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub fn decode_frames(value: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
    let mut out = Vec::new();
    let mut offset = 0usize;

    while offset < value.len() {
        if value.len() - offset < 4 {
            return Err(ProtocolError::Decode("truncated frame header".to_string()));
        }

        let len = u32::from_be_bytes(
            value[offset..offset + 4]
                .try_into()
                .map_err(|_| ProtocolError::Decode("invalid frame header".to_string()))?,
        ) as usize;
        offset += 4;

        if value.len() - offset < len {
            return Err(ProtocolError::Decode("truncated frame payload".to_string()));
        }

        out.push(value[offset..offset + len].to_vec());
        offset += len;
    }

    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuredError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub stage: String,
    #[serde(default)]
    pub details: JsonMap,
}

impl StructuredError {
    pub fn new(code: ErrorCode, stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            stage: stage.into(),
            details: JsonMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    #[serde(rename = "E_UNAUTHORIZED")]
    Unauthorized,
    #[serde(rename = "E_BAD_REQUEST")]
    BadRequest,
    #[serde(rename = "E_BAD_MANIFEST")]
    BadManifest,
    #[serde(rename = "E_BUSY")]
    Busy,
    #[serde(rename = "E_NOT_FOUND")]
    NotFound,
    #[serde(rename = "E_INTERNAL")]
    Internal,
    #[serde(rename = "E_IDEMPOTENCY_CONFLICT")]
    IdempotencyConflict,
    #[serde(rename = "E_RANGE_INVALID")]
    RangeInvalid,
    #[serde(rename = "E_CHUNK_HASH_MISMATCH")]
    ChunkHashMismatch,
    #[serde(rename = "E_ARTIFACT_INCOMPLETE")]
    ArtifactIncomplete,
    #[serde(rename = "E_PRECONDITION_FAILED")]
    PreconditionFailed,
    #[serde(rename = "E_OPERATION_TIMEOUT")]
    OperationTimeout,
    #[serde(rename = "E_ROLLBACK_FAILED")]
    RollbackFailed,
    #[serde(rename = "E_STORAGE_QUOTA")]
    StorageQuota,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelloNegotiateRequest {
    pub compatibility_date: String,
    pub client_version: String,
    #[serde(default)]
    pub required_features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelloNegotiateResponse {
    pub accepted: bool,
    pub server_version: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub limits: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeployPrepareRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub service_type: ServiceType,
    pub target: BTreeMap<String, String>,
    pub artifact_digest: String,
    pub artifact_size: u64,
    pub manifest_digest: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub policy: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeployPrepareResponse {
    pub deploy_id: String,
    pub artifact_status: ArtifactStatus,
    #[serde(default)]
    pub missing_ranges: Vec<ArtifactRange>,
    pub upload_token: String,
    pub session_expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactPushRequest {
    pub deploy_id: String,
    pub offset: u64,
    pub length: u64,
    pub chunk_sha256: String,
    pub upload_token: String,
    pub chunk_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactPushAck {
    #[serde(default)]
    pub received_ranges: Vec<ArtifactRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_missing_range: Option<ArtifactRange>,
    pub accepted_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactCommitRequest {
    pub deploy_id: String,
    pub artifact_digest: String,
    pub artifact_size: u64,
    pub manifest_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactCommitResponse {
    pub artifact_id: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandStartRequest {
    pub request_id: String,
    pub command_type: CommandType,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandStartResponse {
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeployCommandPayload {
    pub deploy_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_current_release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub auto_rollback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunCommandPayload {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StopCommandPayload {
    pub name: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandEvent {
    pub event_type: EventType,
    pub request_id: String,
    pub command_type: CommandType,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StructuredError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateResponse {
    pub request_id: String,
    pub state: OperationState,
    pub stage: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandCancelRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandCancelResponse {
    pub cancellable: bool,
    pub final_state: OperationState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactStatus {
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "partial")]
    Partial,
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServiceType {
    #[serde(rename = "cli")]
    Cli,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "socket")]
    Socket,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandType {
    #[serde(rename = "deploy")]
    Deploy,
    #[serde(rename = "run")]
    Run,
    #[serde(rename = "stop")]
    Stop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventType {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "progress")]
    Progress,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "canceled")]
    Canceled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OperationState {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "canceled")]
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRange {
    pub start: u64,
    pub end: u64,
}

impl ArtifactRange {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub main: String,
    #[serde(rename = "type")]
    pub service_type: ServiceType,
    pub target: JsonMap,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub assets: Vec<ManifestAsset>,
    #[serde(default)]
    pub dependencies: Vec<ManifestDependency>,
    pub hash: ManifestHash,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestAsset {
    pub path: String,
    pub mount: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestDependency {
    pub name: String,
    pub version: String,
    pub source: String,
    pub resolved: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestHash {
    pub algorithm: String,
    pub value: String,
    pub targets: Vec<HashTarget>,
}

impl ManifestHash {
    pub fn validate_targets(&self) -> bool {
        let required = [HashTarget::Wasm, HashTarget::Manifest, HashTarget::Assets]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let actual = self.targets.iter().copied().collect::<BTreeSet<_>>();
        required == actual && self.targets.len() == required.len()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum HashTarget {
    #[serde(rename = "wasm")]
    Wasm,
    #[serde(rename = "manifest")]
    Manifest,
    #[serde(rename = "assets")]
    Assets,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_cbor_envelope() {
        let envelope = Envelope::request(
            MESSAGE_HELLO_NEGOTIATE,
            "r1",
            "c1",
            &HelloNegotiateRequest {
                compatibility_date: "2026-02-10".to_string(),
                client_version: "0.1.0".to_string(),
                required_features: vec!["command-stream".to_string()],
            },
        )
        .expect("envelope should be encoded");

        let buf = to_cbor(&envelope).expect("cbor encode should succeed");
        let decoded: Envelope = from_cbor(&buf).expect("cbor decode should succeed");
        let payload: HelloNegotiateRequest =
            decoded.payload_as().expect("payload should deserialize");

        assert_eq!(decoded.message_type, MESSAGE_HELLO_NEGOTIATE);
        assert_eq!(payload.client_version, "0.1.0");
    }

    #[test]
    fn rejects_legacy_protocol_draft_field() {
        let value = serde_json::json!({
            "protocol_draft": "2026-02-10",
            "client_version": "0.1.0",
            "required_features": [],
        });
        let result = serde_json::from_value::<HelloNegotiateRequest>(value);
        assert!(result.is_err());
    }

    #[test]
    fn returns_error_when_payload_missing() {
        let envelope = Envelope {
            message_type: MESSAGE_STATE_REQUEST.to_string(),
            request_id: "r1".to_string(),
            correlation_id: "c1".to_string(),
            payload: None,
            error: None,
        };

        let result = envelope.payload_as::<StateRequest>();
        assert!(matches!(result, Err(ProtocolError::MissingPayload)));
    }

    #[test]
    fn validates_hash_targets_exactly() {
        let hash = ManifestHash {
            algorithm: "sha256".to_string(),
            value: "deadbeef".to_string(),
            targets: vec![HashTarget::Wasm, HashTarget::Manifest, HashTarget::Assets],
        };
        assert!(hash.validate_targets());

        let invalid = ManifestHash {
            algorithm: "sha256".to_string(),
            value: "deadbeef".to_string(),
            targets: vec![HashTarget::Wasm, HashTarget::Assets],
        };
        assert!(!invalid.validate_targets());
    }

    #[test]
    fn roundtrip_frame_codec() {
        let p1 = b"abc";
        let p2 = b"xyz";
        let mut all = Vec::new();
        all.extend_from_slice(&encode_frame(p1));
        all.extend_from_slice(&encode_frame(p2));

        let frames = decode_frames(&all).expect("frames should decode");
        assert_eq!(frames, vec![b"abc".to_vec(), b"xyz".to_vec()]);
    }
}
