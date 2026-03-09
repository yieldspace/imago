//! Structured protocol error model shared by client and server components.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::validate::{Validate, ValidationError, ensure_required_strings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Stable wire-level error code set.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Rich error payload embedded in protocol envelopes.
pub struct StructuredError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub stage: String,
    pub details: BTreeMap<String, String>,
}

impl Validate for StructuredError {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[(&self.message, "message"), (&self.stage, "stage")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_cbor, to_cbor};

    #[derive(Debug, Serialize)]
    struct InvalidCodeError<'a> {
        code: &'a str,
        message: &'a str,
        retryable: bool,
        stage: &'a str,
        details: BTreeMap<String, String>,
    }

    #[test]
    fn accepts_not_found_error_code() {
        let err = StructuredError {
            code: ErrorCode::NotFound,
            message: "request is not running".to_string(),
            retryable: false,
            stage: "state.request".to_string(),
            details: BTreeMap::new(),
        };

        assert!(err.validate().is_ok());
    }

    #[test]
    fn rejects_unknown_error_code() {
        let encoded = to_cbor(&InvalidCodeError {
            code: "E_UNKNOWN",
            message: "unknown",
            retryable: false,
            stage: "state.request",
            details: BTreeMap::new(),
        })
        .expect("encoding should succeed");

        let decoded = from_cbor::<StructuredError>(&encoded);
        assert!(decoded.is_err());
    }
}
