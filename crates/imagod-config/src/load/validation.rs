use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;

use crate::{
    ImagodConfig, MAX_CHUNK_SIZE_BYTES, is_valid_compatibility_date,
    parse_ed25519_raw_public_key_hex,
};

pub(crate) fn reject_legacy_keys(path: &Path, raw: &toml::Value) -> Result<(), ImagodError> {
    if raw.get("protocol_draft").is_some() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "protocol_draft is no longer supported; use compatibility_date (YYYY-MM-DD)",
        )
        .with_detail("path", path.to_string_lossy())
        .with_detail("legacy_key", "protocol_draft"));
    }

    if let Some(tls) = raw.get("tls").and_then(toml::Value::as_table) {
        for legacy_key in ["server_cert", "client_ca_cert"] {
            if tls.contains_key(legacy_key) {
                return Err(ImagodError::new(
                    ErrorCode::BadRequest,
                    "config.load",
                    format!(
                        "tls.{legacy_key} is no longer supported; use tls.client_public_keys (ed25519 raw public key hex allowlist)"
                    ),
                )
                .with_detail("path", path.to_string_lossy())
                .with_detail("legacy_key", format!("tls.{legacy_key}")));
            }
        }
    }

    Ok(())
}

pub(crate) fn validate(config: &ImagodConfig) -> Result<(), ImagodError> {
    let client_keys =
        parse_unique_public_key_hexes(&config.tls.client_public_keys, "tls.client_public_keys")?;
    parse_unique_public_key_hexes(&config.tls.admin_public_keys, "tls.admin_public_keys")?;
    parse_known_public_key_map(&config.tls.known_public_keys)?;

    for (index, key_hex) in config.tls.admin_public_keys.iter().enumerate() {
        let decoded = parse_ed25519_raw_public_key_hex(key_hex).map_err(|reason| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("tls.admin_public_keys[{index}] {reason}"),
            )
            .with_detail("index", index.to_string())
        })?;
        if client_keys.contains(&decoded) {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("tls.admin_public_keys[{index}] overlaps tls.client_public_keys"),
            )
            .with_detail("index", index.to_string()));
        }
    }

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

    if !(1..=4).contains(&config.runtime.http_worker_count) {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.http_worker_count must be between 1 and 4",
        ));
    }

    if !(1..=16).contains(&config.runtime.http_worker_queue_capacity) {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.http_worker_queue_capacity must be between 1 and 16",
        ));
    }

    if config.runtime.manager_control_read_timeout_ms == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.manager_control_read_timeout_ms must be greater than 0",
        ));
    }

    if config.runtime.max_concurrent_sessions == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.max_concurrent_sessions must be greater than 0",
        ));
    }

    if config.runtime.deploy_stream_timeout_secs == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.deploy_stream_timeout_secs must be greater than 0",
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

fn parse_known_public_key_map(entries: &BTreeMap<String, String>) -> Result<(), ImagodError> {
    for (authority, key_hex) in entries {
        let trimmed = authority.trim();
        if trimmed.is_empty() {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                "tls.known_public_keys authority must not be empty",
            ));
        }
        parse_ed25519_raw_public_key_hex(key_hex).map_err(|reason| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("tls.known_public_keys['{trimmed}'] {reason}"),
            )
            .with_detail("authority", trimmed.to_string())
        })?;
    }
    Ok(())
}

fn parse_unique_public_key_hexes(
    key_hexes: &[String],
    field: &str,
) -> Result<HashSet<[u8; 32]>, ImagodError> {
    let mut seen = HashSet::with_capacity(key_hexes.len());
    for (index, key_hex) in key_hexes.iter().enumerate() {
        let decoded = parse_ed25519_raw_public_key_hex(key_hex).map_err(|reason| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("{field}[{index}] {reason}"),
            )
            .with_detail("index", index.to_string())
        })?;

        if !seen.insert(decoded) {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("{field}[{index}] is duplicated"),
            )
            .with_detail("index", index.to_string()));
        }
    }
    Ok(seen)
}
