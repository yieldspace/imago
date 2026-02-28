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
/// Byte offset range used by resumable upload/ack payloads.
///
/// # Examples
/// ```rust
/// use imago_protocol::{messages::ByteRange, Validate};
///
/// let range = ByteRange { offset: 0, length: 4 };
/// range.validate().expect("positive length is required");
/// ```
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
/// Request payload for `deploy.prepare`.
///
/// # Examples
/// ```rust
/// use std::collections::BTreeMap;
/// use imago_protocol::{messages::DeployPrepareRequest, Validate};
///
/// let request = DeployPrepareRequest {
///     name: "svc-a".to_string(),
///     app_type: "rpc".to_string(),
///     target: BTreeMap::new(),
///     artifact_digest: "sha256:abc".to_string(),
///     artifact_size: 1024,
///     manifest_digest: "sha256:def".to_string(),
///     idempotency_key: "deploy-1".to_string(),
///     policy: BTreeMap::new(),
/// };
/// request.validate().expect("valid deploy.prepare request");
/// ```
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
/// Response payload for `deploy.prepare`.
///
/// # Examples
/// ```rust
/// use imago_protocol::{messages::{ArtifactStatus, ByteRange, DeployPrepareResponse}, Validate};
///
/// let response = DeployPrepareResponse {
///     deploy_id: "deploy-1".to_string(),
///     artifact_status: ArtifactStatus::Partial,
///     missing_ranges: vec![ByteRange { offset: 0, length: 512 }],
///     upload_token: "token-1".to_string(),
///     session_expires_at: "2026-02-27T10:00:00Z".to_string(),
/// };
/// response.validate().expect("partial status requires missing_ranges");
/// ```
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
/// Request payload for `artifact.push`.
///
/// # Examples
/// ```rust
/// use imago_protocol::{messages::{ArtifactPushChunkHeader, ArtifactPushRequest}, Validate};
///
/// let request = ArtifactPushRequest {
///     header: ArtifactPushChunkHeader {
///         deploy_id: "deploy-1".to_string(),
///         offset: 0,
///         length: 4,
///         chunk_sha256: "abcd".to_string(),
///         upload_token: "token-1".to_string(),
///     },
///     chunk: vec![1, 2, 3, 4],
/// };
/// request.validate().expect("chunk is required");
/// ```
pub struct ArtifactPushRequest {
    #[serde(flatten)]
    pub header: ArtifactPushChunkHeader,
    #[serde(with = "serde_bytes")]
    pub chunk: Vec<u8>,
}

impl Validate for ArtifactPushRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.header.validate()?;
        if self.chunk.is_empty() {
            return Err(ValidationError::empty("chunk"));
        }
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

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use std::collections::BTreeMap;

    use super::{
        ArtifactCommitRequest, ArtifactPushAck, ArtifactPushChunkHeader, ArtifactPushRequest,
        ArtifactStatus, ByteRange, DeployPrepareRequest, DeployPrepareResponse,
    };
    use crate::{Validate, from_cbor, to_cbor};

    #[test]
    fn given_deploy_prepare_request_cases__when_validate__then_required_fields_are_enforced() {
        let valid = DeployPrepareRequest {
            name: "svc-a".to_string(),
            app_type: "rpc".to_string(),
            target: BTreeMap::new(),
            artifact_digest: "sha256:abc".to_string(),
            artifact_size: 1024,
            manifest_digest: "sha256:def".to_string(),
            idempotency_key: "deploy-1".to_string(),
            policy: BTreeMap::new(),
        };
        valid.validate().expect("valid request should pass");

        let invalid = DeployPrepareRequest {
            artifact_size: 0,
            ..valid
        };
        let err = invalid
            .validate()
            .expect_err("zero artifact_size should fail");
        assert!(err.to_string().contains("artifact_size"));
    }

    #[test]
    fn given_deploy_prepare_response_cases__when_validate__then_partial_requires_missing_ranges() {
        let invalid = DeployPrepareResponse {
            deploy_id: "deploy-1".to_string(),
            artifact_status: ArtifactStatus::Partial,
            missing_ranges: Vec::new(),
            upload_token: "token".to_string(),
            session_expires_at: "1735689600".to_string(),
        };
        let err = invalid
            .validate()
            .expect_err("partial response without missing_ranges should fail");
        assert!(err.to_string().contains("missing_ranges"));

        let valid = DeployPrepareResponse {
            artifact_status: ArtifactStatus::Partial,
            missing_ranges: vec![ByteRange {
                offset: 0,
                length: 512,
            }],
            ..invalid
        };
        valid
            .validate()
            .expect("partial response with ranges should pass");
    }

    #[test]
    fn given_artifact_push_shapes__when_validate__then_chunk_requirements_are_stable() {
        let valid = ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: "deploy-1".to_string(),
                offset: 0,
                length: 4,
                chunk_sha256: "abcd".to_string(),
                upload_token: "token".to_string(),
            },
            chunk: vec![1, 2, 3, 4],
        };
        valid.validate().expect("valid push request should pass");

        let invalid = ArtifactPushRequest {
            chunk: Vec::new(),
            ..valid
        };
        let err = invalid.validate().expect_err("empty chunk should fail");
        assert!(err.to_string().contains("chunk"));
    }

    #[test]
    fn given_ack_and_commit_cases__when_validate__then_range_and_digest_rules_are_enforced() {
        let invalid_ack = ArtifactPushAck {
            received_ranges: vec![ByteRange {
                offset: 0,
                length: 0,
            }],
            next_missing_range: None,
            accepted_bytes: 0,
        };
        assert!(
            invalid_ack.validate().is_err(),
            "zero length range should fail"
        );

        let invalid_commit = ArtifactCommitRequest {
            deploy_id: "deploy-1".to_string(),
            artifact_digest: "".to_string(),
            artifact_size: 0,
            manifest_digest: "".to_string(),
        };
        assert!(
            invalid_commit.validate().is_err(),
            "commit must require non-empty digests and positive size"
        );
    }

    #[test]
    fn given_wire_payload__when_round_trip__then_artifact_push_request_is_preserved() {
        let request = ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: "deploy-1".to_string(),
                offset: 8,
                length: 4,
                chunk_sha256: "abcd".to_string(),
                upload_token: "token".to_string(),
            },
            chunk: vec![1, 2, 3, 4],
        };
        let encoded = to_cbor(&request).expect("encode should succeed");
        let decoded = from_cbor::<ArtifactPushRequest>(&encoded).expect("decode should succeed");
        assert_eq!(decoded, request);
    }
}
