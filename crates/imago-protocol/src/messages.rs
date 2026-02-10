use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StructuredError;
use crate::validate::{
    Validate, ValidationError, ensure_non_empty, ensure_positive_u64, ensure_uuid_not_nil,
};

pub type StringMap = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    #[serde(rename = "hello.negotiate")]
    HelloNegotiate,
    #[serde(rename = "deploy.prepare")]
    DeployPrepare,
    #[serde(rename = "artifact.push")]
    ArtifactPush,
    #[serde(rename = "artifact.commit")]
    ArtifactCommit,
    #[serde(rename = "command.start")]
    CommandStart,
    #[serde(rename = "command.event")]
    CommandEvent,
    #[serde(rename = "state.request")]
    StateRequest,
    #[serde(rename = "state.response")]
    StateResponse,
    #[serde(rename = "command.cancel")]
    CommandCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactStatus {
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "partial")]
    Partial,
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandType {
    #[serde(rename = "deploy")]
    Deploy,
    #[serde(rename = "run")]
    Run,
    #[serde(rename = "stop")]
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandEventType {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandState {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub offset: u64,
    pub length: u64,
}

impl Validate for ByteRange {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_positive_u64(self.length, "length")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloNegotiateRequest {
    pub compatibility_date: String,
    pub client_version: String,
    pub required_features: Vec<String>,
}

impl Validate for HelloNegotiateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.compatibility_date, "compatibility_date")?;
        ensure_non_empty(&self.client_version, "client_version")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloNegotiateResponse {
    pub accepted: bool,
    pub server_version: String,
    pub features: Vec<String>,
    pub limits: StringMap,
}

impl Validate for HelloNegotiateResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.server_version, "server_version")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployPrepareRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub app_type: String,
    pub target: StringMap,
    pub artifact_digest: String,
    pub artifact_size: u64,
    pub manifest_digest: String,
    pub idempotency_key: String,
    pub policy: StringMap,
}

impl Validate for DeployPrepareRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.name, "name")?;
        ensure_non_empty(&self.app_type, "type")?;
        ensure_non_empty(&self.artifact_digest, "artifact_digest")?;
        ensure_positive_u64(self.artifact_size, "artifact_size")?;
        ensure_non_empty(&self.manifest_digest, "manifest_digest")?;
        ensure_non_empty(&self.idempotency_key, "idempotency_key")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployPrepareResponse {
    pub deploy_id: String,
    pub artifact_status: ArtifactStatus,
    pub missing_ranges: Vec<ByteRange>,
    pub upload_token: String,
    pub session_expires_at: String,
}

impl Validate for DeployPrepareResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.deploy_id, "deploy_id")?;
        ensure_non_empty(&self.upload_token, "upload_token")?;
        ensure_non_empty(&self.session_expires_at, "session_expires_at")?;

        if self.artifact_status == ArtifactStatus::Partial && self.missing_ranges.is_empty() {
            return Err(ValidationError::missing("missing_ranges"));
        }

        for range in &self.missing_ranges {
            range.validate()?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPushChunkHeader {
    pub deploy_id: String,
    pub offset: u64,
    pub length: u64,
    pub chunk_sha256: String,
    pub upload_token: String,
}

impl Validate for ArtifactPushChunkHeader {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.deploy_id, "deploy_id")?;
        ensure_positive_u64(self.length, "length")?;
        ensure_non_empty(&self.chunk_sha256, "chunk_sha256")?;
        ensure_non_empty(&self.upload_token, "upload_token")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPushAck {
    pub received_ranges: Vec<ByteRange>,
    pub next_missing_range: Option<ByteRange>,
    pub accepted_bytes: u64,
}

impl Validate for ArtifactPushAck {
    fn validate(&self) -> Result<(), ValidationError> {
        for range in &self.received_ranges {
            range.validate()?;
        }

        if let Some(next) = &self.next_missing_range {
            next.validate()?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCommitRequest {
    pub deploy_id: String,
    pub artifact_digest: String,
    pub artifact_size: u64,
    pub manifest_digest: String,
}

impl Validate for ArtifactCommitRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.deploy_id, "deploy_id")?;
        ensure_non_empty(&self.artifact_digest, "artifact_digest")?;
        ensure_positive_u64(self.artifact_size, "artifact_size")?;
        ensure_non_empty(&self.manifest_digest, "manifest_digest")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCommitResponse {
    pub artifact_id: String,
    pub verified: bool,
}

impl Validate for ArtifactCommitResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.artifact_id, "artifact_id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandStartRequest {
    pub request_id: Uuid,
    pub command_type: CommandType,
    pub payload: CommandPayload,
}

impl Validate for CommandStartRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;

        match (&self.command_type, &self.payload) {
            (CommandType::Deploy, CommandPayload::Deploy(payload)) => payload.validate(),
            (CommandType::Run, CommandPayload::Run(payload)) => payload.validate(),
            (CommandType::Stop, CommandPayload::Stop(payload)) => payload.validate(),
            _ => Err(ValidationError::invalid(
                "payload",
                "payload does not match command_type",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandStartResponse {
    pub accepted: bool,
}

impl Validate for CommandStartResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandPayload {
    Deploy(DeployCommandPayload),
    Stop(StopCommandPayload),
    Run(RunCommandPayload),
}

impl Validate for CommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Deploy(payload) => payload.validate(),
            Self::Run(payload) => payload.validate(),
            Self::Stop(payload) => payload.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployCommandPayload {
    pub deploy_id: String,
    pub expected_current_release: String,
    pub restart_policy: String,
    #[serde(default = "default_true")]
    pub auto_rollback: bool,
}

impl Validate for DeployCommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.deploy_id, "deploy_id")?;
        ensure_non_empty(&self.expected_current_release, "expected_current_release")?;
        ensure_non_empty(&self.restart_policy, "restart_policy")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCommandPayload {
    pub name: String,
}

impl Validate for RunCommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.name, "name")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopCommandPayload {
    pub name: String,
    pub force: bool,
}

impl Validate for StopCommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.name, "name")
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEvent {
    pub event_type: CommandEventType,
    pub request_id: Uuid,
    pub command_type: CommandType,
    pub timestamp: String,
    pub stage: Option<String>,
    pub error: Option<StructuredError>,
}

impl Validate for CommandEvent {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;
        ensure_non_empty(&self.timestamp, "timestamp")?;

        if self.event_type == CommandEventType::Progress {
            let stage = self
                .stage
                .as_deref()
                .ok_or(ValidationError::missing("stage"))?;
            ensure_non_empty(stage, "stage")?;
        }

        if self.event_type == CommandEventType::Failed {
            let err = self
                .error
                .as_ref()
                .ok_or(ValidationError::missing("error"))?;
            err.validate()?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRequest {
    pub request_id: Uuid,
}

impl Validate for StateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateResponse {
    pub request_id: Uuid,
    pub state: CommandState,
    pub stage: String,
    pub updated_at: String,
}

impl Validate for StateResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;
        match self.state {
            CommandState::Accepted | CommandState::Running => {}
            CommandState::Succeeded | CommandState::Failed | CommandState::Canceled => {
                return Err(ValidationError::invalid(
                    "state",
                    "terminal states are not allowed for state.response",
                ));
            }
        }
        ensure_non_empty(&self.stage, "stage")?;
        ensure_non_empty(&self.updated_at, "updated_at")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCancelRequest {
    pub request_id: Uuid,
}

impl Validate for CommandCancelRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCancelResponse {
    pub cancellable: bool,
    pub final_state: CommandState,
}

impl Validate for CommandCancelResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_cbor, to_cbor};

    fn sample_request_id() -> Uuid {
        Uuid::from_u128(0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA)
    }

    fn sample_deploy_prepare_request() -> DeployPrepareRequest {
        DeployPrepareRequest {
            name: "syslog-forwarder".to_string(),
            app_type: "socket".to_string(),
            target: StringMap::new(),
            artifact_digest: "sha256:1111".to_string(),
            artifact_size: 1024,
            manifest_digest: "sha256:2222".to_string(),
            idempotency_key: "deploy-1".to_string(),
            policy: StringMap::new(),
        }
    }

    #[test]
    fn hello_negotiate_round_trip_and_validate() {
        let request = HelloNegotiateRequest {
            compatibility_date: "2026-02-10".to_string(),
            client_version: "0.1.0".to_string(),
            required_features: vec!["resumable-upload".to_string()],
        };

        request.validate().expect("request should be valid");
        let encoded = to_cbor(&request).expect("encoding should succeed");
        let decoded: HelloNegotiateRequest = from_cbor(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, request);
    }

    #[derive(Debug, Serialize)]
    struct HelloNegotiateMissingRequiredFeatures<'a> {
        compatibility_date: &'a str,
        client_version: &'a str,
    }

    #[test]
    fn hello_negotiate_rejects_missing_required_field() {
        let encoded = to_cbor(&HelloNegotiateMissingRequiredFeatures {
            compatibility_date: "2026-02-10",
            client_version: "0.1.0",
        })
        .expect("encoding should succeed");

        let decoded = from_cbor::<HelloNegotiateRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[derive(Debug, Serialize)]
    struct DeployPrepareMissingIdempotency<'a> {
        name: &'a str,
        #[serde(rename = "type")]
        app_type: &'a str,
        target: StringMap,
        artifact_digest: &'a str,
        artifact_size: u64,
        manifest_digest: &'a str,
        policy: StringMap,
    }

    #[test]
    fn deploy_prepare_rejects_missing_idempotency_key() {
        let encoded = to_cbor(&DeployPrepareMissingIdempotency {
            name: "syslog-forwarder",
            app_type: "socket",
            target: StringMap::new(),
            artifact_digest: "sha256:1111",
            artifact_size: 2048,
            manifest_digest: "sha256:2222",
            policy: StringMap::new(),
        })
        .expect("encoding should succeed");

        let decoded = from_cbor::<DeployPrepareRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn deploy_prepare_rejects_empty_idempotency_key() {
        let mut request = sample_deploy_prepare_request();
        request.idempotency_key.clear();
        assert!(request.validate().is_err());
    }

    #[test]
    fn artifact_push_validates_range_and_hash_header() {
        let header = ArtifactPushChunkHeader {
            deploy_id: "dep-1".to_string(),
            offset: 0,
            length: 0,
            chunk_sha256: "".to_string(),
            upload_token: "token".to_string(),
        };
        assert!(header.validate().is_err());

        let ack = ArtifactPushAck {
            received_ranges: vec![ByteRange {
                offset: 0,
                length: 0,
            }],
            next_missing_range: None,
            accepted_bytes: 0,
        };
        assert!(ack.validate().is_err());
    }

    #[test]
    fn artifact_commit_rejects_missing_required_values() {
        let request = ArtifactCommitRequest {
            deploy_id: "dep-1".to_string(),
            artifact_digest: "".to_string(),
            artifact_size: 0,
            manifest_digest: "".to_string(),
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn command_start_validates_each_payload_type() {
        let deploy = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Deploy,
            payload: CommandPayload::Deploy(DeployCommandPayload {
                deploy_id: "dep-1".to_string(),
                expected_current_release: "rel-1".to_string(),
                restart_policy: "never".to_string(),
                auto_rollback: true,
            }),
        };
        assert!(deploy.validate().is_ok());

        let run = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Run,
            payload: CommandPayload::Run(RunCommandPayload {
                name: "syslog-forwarder".to_string(),
            }),
        };
        assert!(run.validate().is_ok());

        let stop = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Stop,
            payload: CommandPayload::Stop(StopCommandPayload {
                name: "syslog-forwarder".to_string(),
                force: false,
            }),
        };
        assert!(stop.validate().is_ok());
    }

    #[test]
    fn command_start_rejects_payload_command_mismatch() {
        let request = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Run,
            payload: CommandPayload::Deploy(DeployCommandPayload {
                deploy_id: "dep-1".to_string(),
                expected_current_release: "rel-1".to_string(),
                restart_policy: "never".to_string(),
                auto_rollback: true,
            }),
        };
        assert!(request.validate().is_err());
    }

    #[derive(Debug, Serialize)]
    struct DeployPayloadWithoutAutoRollback<'a> {
        deploy_id: &'a str,
        expected_current_release: &'a str,
        restart_policy: &'a str,
    }

    #[test]
    fn deploy_payload_defaults_auto_rollback_to_true() {
        let encoded = to_cbor(&DeployPayloadWithoutAutoRollback {
            deploy_id: "dep-1",
            expected_current_release: "rel-1",
            restart_policy: "never",
        })
        .expect("encoding should succeed");

        let decoded: DeployCommandPayload = from_cbor(&encoded).expect("decoding should succeed");
        assert!(decoded.auto_rollback);
    }

    #[test]
    fn command_event_enforces_progress_and_failed_requirements() {
        let progress = CommandEvent {
            event_type: CommandEventType::Progress,
            request_id: sample_request_id(),
            command_type: CommandType::Deploy,
            timestamp: "2026-02-10T00:00:00Z".to_string(),
            stage: None,
            error: None,
        };
        assert!(progress.validate().is_err());

        let failed = CommandEvent {
            event_type: CommandEventType::Failed,
            request_id: sample_request_id(),
            command_type: CommandType::Deploy,
            timestamp: "2026-02-10T00:00:01Z".to_string(),
            stage: Some("commit".to_string()),
            error: None,
        };
        assert!(failed.validate().is_err());
    }

    #[test]
    fn state_request_and_response_validate_required_fields() {
        let invalid_request = StateRequest {
            request_id: Uuid::nil(),
        };
        assert!(invalid_request.validate().is_err());

        let invalid_response = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Running,
            stage: "".to_string(),
            updated_at: "".to_string(),
        };
        assert!(invalid_response.validate().is_err());
    }

    #[test]
    fn state_response_rejects_terminal_states() {
        let succeeded = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Succeeded,
            stage: "done".to_string(),
            updated_at: "2026-02-10T00:00:00Z".to_string(),
        };
        assert!(succeeded.validate().is_err());

        let failed = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Failed,
            stage: "rollback".to_string(),
            updated_at: "2026-02-10T00:00:01Z".to_string(),
        };
        assert!(failed.validate().is_err());

        let canceled = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Canceled,
            stage: "cancel".to_string(),
            updated_at: "2026-02-10T00:00:02Z".to_string(),
        };
        assert!(canceled.validate().is_err());

        let running = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Running,
            stage: "deploying".to_string(),
            updated_at: "2026-02-10T00:00:03Z".to_string(),
        };
        assert!(running.validate().is_ok());
    }

    #[derive(Debug, Serialize)]
    struct CommandCancelMissingFinalState {
        cancellable: bool,
    }

    #[test]
    fn command_cancel_validates_request_and_response_shape() {
        let invalid_request = CommandCancelRequest {
            request_id: Uuid::nil(),
        };
        assert!(invalid_request.validate().is_err());

        let encoded = to_cbor(&CommandCancelMissingFinalState { cancellable: true })
            .expect("encoding should succeed");
        let decoded = from_cbor::<CommandCancelResponse>(&encoded);
        assert!(decoded.is_err());
    }
}
