use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::validate::{Validate, ValidationError, ensure_non_empty, ensure_uuid_not_nil};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogRequest {
    pub name: Option<String>,
    pub follow: bool,
    pub tail_lines: u32,
}

impl Validate for LogRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(name) = self.name.as_deref() {
            ensure_non_empty(name, "name")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogStreamKind {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "stderr")]
    Stderr,
    #[serde(rename = "composite")]
    Composite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogChunk {
    pub request_id: Uuid,
    pub seq: u64,
    pub name: String,
    pub stream_kind: LogStreamKind,
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub is_last: bool,
}

impl Validate for LogChunk {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;
        ensure_non_empty(&self.name, "name")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogErrorCode {
    #[serde(rename = "process_not_found")]
    ProcessNotFound,
    #[serde(rename = "process_not_running")]
    ProcessNotRunning,
    #[serde(rename = "permission_denied")]
    PermissionDenied,
    #[serde(rename = "internal")]
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogError {
    pub code: LogErrorCode,
    pub message: String,
}

impl Validate for LogError {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.message, "message")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogEnd {
    pub request_id: Uuid,
    pub seq: u64,
    pub error: Option<LogError>,
}

impl Validate for LogEnd {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;
        if let Some(error) = &self.error {
            error.validate()?;
        }

        Ok(())
    }
}
