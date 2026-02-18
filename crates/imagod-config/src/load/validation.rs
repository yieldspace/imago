use std::path::Path;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;

use crate::{ImagodConfig, MAX_CHUNK_SIZE_BYTES, is_valid_compatibility_date};

pub(crate) fn reject_legacy_keys(path: &Path, raw: &toml::Value) -> Result<(), ImagodError> {
    if raw.get("protocol_draft").is_none() {
        return Ok(());
    }

    Err(ImagodError::new(
        ErrorCode::BadRequest,
        "config.load",
        "protocol_draft is no longer supported; use compatibility_date (YYYY-MM-DD)",
    )
    .with_detail("path", path.to_string_lossy())
    .with_detail("legacy_key", "protocol_draft"))
}

pub(crate) fn validate(config: &ImagodConfig) -> Result<(), ImagodError> {
    if !is_valid_compatibility_date(&config.compatibility_date) {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "compatibility_date must be in YYYY-MM-DD format",
        )
        .with_detail("compatibility_date", config.compatibility_date.clone()));
    }

    if config.runtime.stop_grace_timeout_secs == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.stop_grace_timeout_secs must be greater than 0",
        ));
    }

    if config.runtime.epoch_tick_interval_ms == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.epoch_tick_interval_ms must be greater than 0",
        ));
    }

    if config.runtime.runner_ready_timeout_secs == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.runner_ready_timeout_secs must be greater than 0",
        ));
    }

    if config.runtime.runner_log_buffer_bytes == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.runner_log_buffer_bytes must be greater than 0",
        ));
    }

    if config.runtime.max_artifact_size_bytes == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.max_artifact_size_bytes must be greater than 0",
        ));
    }

    if config.runtime.chunk_size == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.chunk_size must be greater than 0",
        ));
    }

    if config.runtime.chunk_size > MAX_CHUNK_SIZE_BYTES {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!(
                "runtime.chunk_size must be less than or equal to {}",
                MAX_CHUNK_SIZE_BYTES
            ),
        ));
    }

    if config.runtime.max_inflight_chunks == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.max_inflight_chunks must be greater than 0",
        ));
    }

    Ok(())
}
