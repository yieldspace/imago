//! Common shared types used by internal `imagod-*` crates.

use std::collections::BTreeMap;

use imagod_spec::{ErrorCode, IpcErrorPayload, StructuredError};
use thiserror::Error;

mod builders;

/// Default Wasmtime linear-memory reservation size in bytes.
pub const DEFAULT_WASM_MEMORY_RESERVATION_BYTES: u64 = 64 * 1024 * 1024;
/// Default Wasmtime linear-memory growth reservation size in bytes.
pub const DEFAULT_WASM_MEMORY_RESERVATION_FOR_GROWTH_BYTES: u64 = 16 * 1024 * 1024;
/// Default Wasmtime linear-memory guard size in bytes.
pub const DEFAULT_WASM_MEMORY_GUARD_SIZE_BYTES: u64 = 64 * 1024;
/// Default flag for guard pages before linear memory.
pub const DEFAULT_WASM_GUARD_BEFORE_LINEAR_MEMORY: bool = false;
/// Default flag for Wasmtime parallel compilation.
pub const DEFAULT_WASM_PARALLEL_COMPILATION: bool = false;

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

    /// Returns a stable symbolic name for the wire-oriented protocol error code.
    pub fn code_name(&self) -> &'static str {
        match self.code {
            ErrorCode::Unauthorized => "Unauthorized",
            ErrorCode::BadRequest => "BadRequest",
            ErrorCode::BadManifest => "BadManifest",
            ErrorCode::Busy => "Busy",
            ErrorCode::NotFound => "NotFound",
            ErrorCode::Internal => "Internal",
            ErrorCode::IdempotencyConflict => "IdempotencyConflict",
            ErrorCode::RangeInvalid => "RangeInvalid",
            ErrorCode::ChunkHashMismatch => "ChunkHashMismatch",
            ErrorCode::ArtifactIncomplete => "ArtifactIncomplete",
            ErrorCode::PreconditionFailed => "PreconditionFailed",
            ErrorCode::OperationTimeout => "OperationTimeout",
            ErrorCode::RollbackFailed => "RollbackFailed",
            ErrorCode::StorageQuota => "StorageQuota",
        }
    }

    /// Converts this value into the IPC wire error shape.
    pub fn to_ipc_error_payload(&self) -> IpcErrorPayload {
        IpcErrorPayload {
            code: self.code,
            stage: self.stage.clone(),
            message: self.message.clone(),
        }
    }
}

impl From<IpcErrorPayload> for ImagodError {
    fn from(value: IpcErrorPayload) -> Self {
        Self::new(value.code, value.stage, value.message)
    }
}
