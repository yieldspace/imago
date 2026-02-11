use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ImagodError;
use imago_protocol::ErrorCode;

const MAX_CHUNK_SIZE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct ImagodConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    pub tls: TlsConfig,
    #[serde(default = "default_storage_root")]
    pub storage_root: PathBuf,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default = "default_server_version")]
    pub server_version: String,
    #[serde(default = "default_compatibility_date")]
    pub compatibility_date: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub server_cert: PathBuf,
    pub server_key: PathBuf,
    pub client_ca_cert: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_max_inflight_chunks")]
    pub max_inflight_chunks: usize,
    #[serde(default = "default_max_artifact_size_bytes")]
    pub max_artifact_size_bytes: u64,
    #[serde(default = "default_upload_session_ttl_secs")]
    pub upload_session_ttl_secs: u64,
    #[serde(default = "default_stop_grace_timeout_secs")]
    pub stop_grace_timeout_secs: u64,
    #[serde(default = "default_epoch_tick_interval_ms")]
    pub epoch_tick_interval_ms: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            chunk_size: default_chunk_size(),
            max_inflight_chunks: default_max_inflight_chunks(),
            max_artifact_size_bytes: default_max_artifact_size_bytes(),
            upload_session_ttl_secs: default_upload_session_ttl_secs(),
            stop_grace_timeout_secs: default_stop_grace_timeout_secs(),
            epoch_tick_interval_ms: default_epoch_tick_interval_ms(),
        }
    }
}

impl ImagodConfig {
    pub fn load(path: &Path) -> Result<Self, ImagodError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("config read failed: {e}"),
            )
            .with_detail("path", path.to_string_lossy())
        })?;
        let raw: toml::Value = toml::from_str(&content).map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("config parse failed: {e}"),
            )
            .with_detail("path", path.to_string_lossy())
        })?;

        if raw.get("protocol_draft").is_some() {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                "protocol_draft is no longer supported; use compatibility_date (YYYY-MM-DD)",
            )
            .with_detail("path", path.to_string_lossy())
            .with_detail("legacy_key", "protocol_draft"));
        }

        let config: Self = raw.clone().try_into().map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("config decode failed: {e}"),
            )
            .with_detail("path", path.to_string_lossy())
        })?;

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

        Ok(config)
    }
}

pub fn resolve_config_path(cli_path: Option<PathBuf>) -> PathBuf {
    if let Some(path) = cli_path {
        return path;
    }
    if let Ok(path) = std::env::var("IMAGOD_CONFIG") {
        return PathBuf::from(path);
    }
    PathBuf::from("/etc/imago/imagod.toml")
}

fn default_listen_addr() -> String {
    "[::]:4443".to_string()
}

fn default_storage_root() -> PathBuf {
    PathBuf::from("/etc/imago")
}

fn default_server_version() -> String {
    "imagod/0.1.0".to_string()
}

fn default_compatibility_date() -> String {
    "2026-02-10".to_string()
}

fn default_chunk_size() -> usize {
    1024 * 1024
}

fn default_max_inflight_chunks() -> usize {
    16
}

fn default_max_artifact_size_bytes() -> u64 {
    64 * 1024 * 1024
}

fn default_upload_session_ttl_secs() -> u64 {
    15 * 60
}

fn default_stop_grace_timeout_secs() -> u64 {
    30
}

fn default_epoch_tick_interval_ms() -> u64 {
    50
}

fn is_valid_compatibility_date(value: &str) -> bool {
    if value.len() != 10 {
        return false;
    }

    let bytes = value.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    if !bytes
        .iter()
        .enumerate()
        .all(|(index, b)| matches!(index, 4 | 7) || b.is_ascii_digit())
    {
        return false;
    }

    let year_ok = value[0..4].parse::<u32>().is_ok();
    let month_ok = value[5..7]
        .parse::<u32>()
        .map(|m| (1..=12).contains(&m))
        .unwrap_or(false);
    let day_ok = value[8..10]
        .parse::<u32>()
        .map(|d| (1..=31).contains(&d))
        .unwrap_or(false);

    year_ok && month_ok && day_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn defaults_compatibility_date_when_missing() {
        let path = write_temp_config(
            "defaults_compatibility_date_when_missing",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"
"#,
        );

        let config = ImagodConfig::load(&path).expect("config should load");
        assert_eq!(config.compatibility_date, "2026-02-10");
        assert_eq!(config.runtime.max_artifact_size_bytes, 64 * 1024 * 1024);

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_legacy_protocol_draft_key() {
        let path = write_temp_config(
            "rejects_legacy_protocol_draft_key",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
protocol_draft = "imago-mvp-v1"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject legacy key");
        let message = err.to_string();
        assert!(message.contains("protocol_draft"));
        assert!(message.contains("compatibility_date"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_runtime_intervals() {
        let path = write_temp_config(
            "rejects_zero_runtime_intervals",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"

[runtime]
stop_grace_timeout_secs = 0
epoch_tick_interval_ms = 0
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject zero runtime values");
        let message = err.to_string();
        assert!(
            message.contains("stop_grace_timeout_secs")
                || message.contains("epoch_tick_interval_ms")
        );

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_max_artifact_size() {
        let path = write_temp_config(
            "rejects_zero_max_artifact_size",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"

[runtime]
max_artifact_size_bytes = 0
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject zero artifact size");
        assert!(err.to_string().contains("max_artifact_size_bytes"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_chunk_size() {
        let path = write_temp_config(
            "rejects_zero_chunk_size",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"

[runtime]
chunk_size = 0
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject zero chunk size");
        assert!(err.to_string().contains("runtime.chunk_size"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_max_inflight_chunks() {
        let path = write_temp_config(
            "rejects_zero_max_inflight_chunks",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"

[runtime]
max_inflight_chunks = 0
"#,
        );

        let err =
            ImagodConfig::load(&path).expect_err("config should reject zero max_inflight_chunks");
        assert!(err.to_string().contains("runtime.max_inflight_chunks"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_chunk_size_over_8mib() {
        let path = write_temp_config(
            "rejects_chunk_size_over_8mib",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"

[runtime]
chunk_size = 8388609
"#,
        );

        let err =
            ImagodConfig::load(&path).expect_err("config should reject chunk_size above 8MiB");
        assert!(err.to_string().contains("runtime.chunk_size"));

        cleanup_temp_path(path);
    }

    fn write_temp_config(test_name: &str, body: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let base_dir = std::env::temp_dir().join(format!("imagod-config-tests-{test_name}-{ts}"));
        std::fs::create_dir_all(&base_dir).expect("temp dir creation should succeed");
        let path = base_dir.join("imagod.toml");
        std::fs::write(&path, body).expect("config write should succeed");
        path
    }

    fn cleanup_temp_path(path: PathBuf) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
