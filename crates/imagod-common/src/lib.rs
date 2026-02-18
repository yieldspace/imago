//! Common shared types used by internal `imagod-*` crates.

use std::collections::BTreeMap;

use imago_protocol::{ErrorCode, StructuredError};
use thiserror::Error;

mod builders;

#[derive(Debug, Error)]
#[error("{code:?} at {stage}: {message}")]
/// Rich internal error type carried across `imagod` components.
pub struct ImagodError {
    /// Stable protocol error code.
    pub code: ErrorCode,
    /// Logical processing stage where the error was raised.
    pub stage: String,
    /// Human-readable error summary.
    pub message: String,
    /// Whether retrying the same operation may succeed.
    pub retryable: bool,
    /// Optional structured details for logs and wire errors.
    pub details: BTreeMap<String, String>,
}

impl ImagodError {
    /// Creates a new error with default retryable=false and no details.
    pub fn new(code: ErrorCode, stage: impl Into<String>, message: impl Into<String>) -> Self {
        builders::new_error(code, stage, message)
    }

    /// Sets the retryable flag.
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Appends one key/value detail entry.
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        builders::insert_detail(&mut self.details, key, value);
        self
    }

    /// Converts this value into the protocol wire error shape.
    pub fn to_structured(&self) -> StructuredError {
        StructuredError {
            code: self.code,
            message: self.message.clone(),
            retryable: self.retryable,
            stage: self.stage.clone(),
            details: self.details.clone(),
        }
    }
}
