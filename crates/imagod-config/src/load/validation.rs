//! Semantic validation helpers for decoded daemon configuration.
//!
//! This module enforces cross-field and bounds constraints that are not
//! expressed by TOML type decoding alone.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_common::{BUILTIN_NATIVE_PLUGIN_DESCRIPTORS, is_builtin_native_plugin_package_name};

use crate::{
    ImagodConfig, MAX_CHUNK_SIZE_BYTES, MAX_HTTP_QUEUE_MEMORY_BUDGET_BYTES, RuntimeFeatures,
    parse_ed25519_raw_public_key_hex,
};

pub(crate) fn reject_legacy_keys(path: &Path, raw: &toml::Value) -> Result<(), ImagodError> {
    if raw.get("protocol_draft").is_some() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "protocol_draft is no longer supported; protocol compatibility is negotiated by hello.negotiate client_version",
        )
        .with_detail("path", path.to_string_lossy())
        .with_detail("legacy_key", "protocol_draft"));
    }
    if raw.get("compatibility_date").is_some() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "compatibility_date is no longer supported; protocol compatibility is negotiated by hello.negotiate client_version",
        )
        .with_detail("path", path.to_string_lossy())
        .with_detail("legacy_key", "compatibility_date"));
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
    if config.control_socket_path.as_os_str().is_empty() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "control_socket_path must not be empty",
        ));
    }

    if !config.control_socket_path.is_absolute() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!(
                "control_socket_path must be an absolute path: {}",
                config.control_socket_path.display()
            ),
        ));
    }

    let client_keys =
        parse_unique_public_key_hexes(&config.tls.client_public_keys, "tls.client_public_keys")?;
    parse_unique_public_key_hexes(&config.tls.admin_public_keys, "tls.admin_public_keys")?;
    parse_known_public_key_map(&config.tls.known_public_keys)?;
    validate_runtime_features(&config.runtime.features)?;

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

    if config.runtime.wasm_memory_reservation_bytes == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.wasm_memory_reservation_bytes must be greater than 0",
        ));
    }

    if config.runtime.wasm_memory_reservation_for_growth_bytes == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.wasm_memory_reservation_for_growth_bytes must be greater than 0",
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

    if config.runtime.retained_logs_capacity_bytes == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.retained_logs_capacity_bytes must be greater than 0",
        ));
    }

    if config.runtime.committed_session_ttl_secs == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.committed_session_ttl_secs must be greater than 0",
        ));
    }

    if config.runtime.max_committed_sessions == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.max_committed_sessions must be greater than 0",
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

    if config.runtime.http_queue_memory_budget_bytes == 0
        || config.runtime.http_queue_memory_budget_bytes > MAX_HTTP_QUEUE_MEMORY_BUDGET_BYTES
    {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!(
                "runtime.http_queue_memory_budget_bytes must be in range 1..={MAX_HTTP_QUEUE_MEMORY_BUDGET_BYTES}"
            ),
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

    if config.runtime.transport_keepalive_interval_secs == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.transport_keepalive_interval_secs must be greater than 0",
        ));
    }

    if config.runtime.transport_max_idle_timeout_secs == 0 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.transport_max_idle_timeout_secs must be greater than 0",
        ));
    }

    if config.runtime.transport_keepalive_interval_secs
        >= config.runtime.transport_max_idle_timeout_secs
    {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            "runtime.transport_keepalive_interval_secs must be less than runtime.transport_max_idle_timeout_secs",
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

fn validate_runtime_features(features: &RuntimeFeatures) -> Result<(), ImagodError> {
    let RuntimeFeatures::Packages(package_names) = features else {
        return Ok(());
    };

    let supported = BUILTIN_NATIVE_PLUGIN_DESCRIPTORS
        .iter()
        .map(|descriptor| descriptor.package_name)
        .collect::<Vec<_>>()
        .join(", ");

    for (index, package_name) in package_names.iter().enumerate() {
        if is_builtin_native_plugin_package_name(package_name) {
            continue;
        }

        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!("runtime.features[{index}] must be one of: {supported} (got '{package_name}')"),
        )
        .with_detail("index", index.to_string())
        .with_detail("value", package_name));
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

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    use std::{collections::BTreeMap, path::PathBuf};

    use crate::{DEFAULT_CONTROL_SOCKET_PATH, RuntimeConfig, TlsConfig};

    use super::*;

    fn sample_config() -> ImagodConfig {
        ImagodConfig {
            listen_addr: "127.0.0.1:4443".to_string(),
            control_socket_path: PathBuf::from(DEFAULT_CONTROL_SOCKET_PATH),
            tls: TlsConfig {
                server_key: PathBuf::from("/tmp/server.key"),
                admin_public_keys: Vec::new(),
                client_public_keys: Vec::new(),
                known_public_keys: BTreeMap::new(),
            },
            storage_root: PathBuf::from("/tmp/imago-storage"),
            runtime: RuntimeConfig::default(),
            server_version: "imagod/test".to_string(),
        }
    }

    #[test]
    fn given_relative_control_socket_path__when_validate__then_bad_request_is_returned() {
        let mut config = sample_config();
        config.control_socket_path = PathBuf::from("imagod.sock");

        let err = validate(&config).expect_err("relative control socket path must fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, "config.load");
        assert!(
            err.message
                .contains("control_socket_path must be an absolute path")
        );
    }

    #[test]
    fn given_default_control_socket_path__when_validate__then_config_is_accepted() {
        validate(&sample_config()).expect("default control socket path should validate");
    }
}
