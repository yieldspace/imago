use serde::{Deserialize, Serialize};

use crate::validate::{
    Validate, ValidationError, ensure_non_empty, ensure_positive_u64, ensure_required_strings,
};

use super::StringMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactStatus {
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "partial")]
    Partial,
    #[serde(rename = "complete")]
    Complete,
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
        ensure_required_strings(&[
            (&self.name, "name"),
            (&self.app_type, "type"),
            (&self.artifact_digest, "artifact_digest"),
            (&self.manifest_digest, "manifest_digest"),
            (&self.idempotency_key, "idempotency_key"),
        ])?;
        ensure_positive_u64(self.artifact_size, "artifact_size")
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
        ensure_required_strings(&[
            (&self.deploy_id, "deploy_id"),
            (&self.upload_token, "upload_token"),
            (&self.session_expires_at, "session_expires_at"),
        ])?;

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
        ensure_required_strings(&[
            (&self.deploy_id, "deploy_id"),
            (&self.chunk_sha256, "chunk_sha256"),
            (&self.upload_token, "upload_token"),
        ])?;
        ensure_positive_u64(self.length, "length")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPushRequest {
    #[serde(flatten)]
    pub header: ArtifactPushChunkHeader,
    pub chunk_b64: String,
}

impl Validate for ArtifactPushRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.header.validate()?;
        ensure_non_empty(&self.chunk_b64, "chunk_b64")
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
        ensure_required_strings(&[
            (&self.deploy_id, "deploy_id"),
            (&self.artifact_digest, "artifact_digest"),
            (&self.manifest_digest, "manifest_digest"),
        ])?;
        ensure_positive_u64(self.artifact_size, "artifact_size")
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
