use std::collections::BTreeMap;

use imago_protocol::{ErrorCode, StructuredError};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{code:?} at {stage}: {message}")]
pub struct ImagodError {
    pub code: ErrorCode,
    pub stage: String,
    pub message: String,
    pub retryable: bool,
    pub details: BTreeMap<String, String>,
}

impl ImagodError {
    pub fn new(code: ErrorCode, stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            stage: stage.into(),
            message: message.into(),
            retryable: false,
            details: BTreeMap::new(),
        }
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

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
