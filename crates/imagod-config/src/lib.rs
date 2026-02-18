//! Configuration loader and validation for the `imagod` manager process.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use imagod_common::ImagodError;

mod load;

const MAX_CHUNK_SIZE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
/// Root runtime configuration loaded from `imagod.toml`.
pub struct ImagodConfig {
    #[serde(default = "default_listen_addr")]
    /// WebTransport listen address.
    pub listen_addr: String,
    /// TLS certificate and key paths.
    pub tls: TlsConfig,
    #[serde(default = "default_storage_root")]
    /// Storage root used for artifacts, runtime state, and releases.
    pub storage_root: PathBuf,
    #[serde(default)]
    /// Runtime limits and process-control knobs.
    pub runtime: RuntimeConfig,
    #[serde(default = "default_server_version")]
    /// Server version reported via negotiate response.
    pub server_version: String,
    #[serde(default = "default_compatibility_date")]
    /// Compatibility key used by hello negotiation.
    pub compatibility_date: String,
}

#[derive(Debug, Clone, Deserialize)]
/// TLS material locations for mTLS server startup.
pub struct TlsConfig {
    /// Server certificate chain in PEM format.
    pub server_cert: PathBuf,
    /// Server private key in PEM format.
    pub server_key: PathBuf,
    /// Client CA certificate bundle for client verification.
    pub client_ca_cert: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
/// Runtime limits and timeouts applied by manager and runner.
pub struct RuntimeConfig {
    #[serde(default = "default_chunk_size")]
    /// Maximum chunk size accepted by `artifact.push`.
    pub chunk_size: usize,
    #[serde(default = "default_max_inflight_chunks")]
    /// Max concurrent chunk writes per upload session.
    pub max_inflight_chunks: usize,
    #[serde(default = "default_max_artifact_size_bytes")]
    /// Upper bound for accepted artifact archive size.
    pub max_artifact_size_bytes: u64,
    #[serde(default = "default_upload_session_ttl_secs")]
    /// Upload session time-to-live in seconds.
    pub upload_session_ttl_secs: u64,
    #[serde(default = "default_stop_grace_timeout_secs")]
    /// Grace period for service stop before forced kill.
    pub stop_grace_timeout_secs: u64,
    #[serde(default = "default_runner_ready_timeout_secs")]
    /// Deadline for waiting runner-ready handshake.
    pub runner_ready_timeout_secs: u64,
    #[serde(default = "default_runner_log_buffer_bytes")]
    /// Total per-runner log ring capacity in bytes.
    pub runner_log_buffer_bytes: usize,
    #[serde(default = "default_epoch_tick_interval_ms")]
    /// Runner epoch-tick interval in milliseconds.
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
            runner_ready_timeout_secs: default_runner_ready_timeout_secs(),
            runner_log_buffer_bytes: default_runner_log_buffer_bytes(),
            epoch_tick_interval_ms: default_epoch_tick_interval_ms(),
        }
    }
}

impl ImagodConfig {
    /// Loads and validates `imagod.toml` from disk.
    ///
    /// Returns `ImagodError` with `ErrorCode::BadRequest` when decode or
    /// validation fails.
    pub fn load(path: &Path) -> Result<Self, ImagodError> {
        let content = load::io::read_to_string(path)?;
        let raw = load::parsing::parse(path, &content)?;
        load::validation::reject_legacy_keys(path, &raw)?;
        let config = load::parsing::decode(path, raw)?;
        load::validation::validate(&config)?;
        Ok(config)
    }
}

/// Resolves the config file path in priority order:
/// CLI `--config`, `IMAGOD_CONFIG`, then default system path.
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
    resolve_default_storage_root(
        std::env::consts::OS,
        option_env!("IMAGOD_STORAGE_ROOT_DEFAULT"),
    )
}

fn resolve_default_storage_root(target_os: &str, build_override: Option<&str>) -> PathBuf {
    if let Some(path) = build_override
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }

    match target_os {
        "linux" => PathBuf::from("/var/lib/imago"),
        "macos" => PathBuf::from("/usr/local/var/imago"),
        "windows" => PathBuf::from(r"C:\ProgramData\imago"),
        _ => PathBuf::from("/var/lib/imago"),
    }
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

fn default_runner_ready_timeout_secs() -> u64 {
    3
}

fn default_runner_log_buffer_bytes() -> usize {
    256 * 1024
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
server_version = "imagod/test"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"
"#,
        );

        let config = ImagodConfig::load(&path).expect("config should load");
        assert_eq!(
            config.storage_root,
            resolve_default_storage_root(
                std::env::consts::OS,
                option_env!("IMAGOD_STORAGE_ROOT_DEFAULT")
            )
        );
        assert_eq!(config.compatibility_date, "2026-02-10");
        assert_eq!(config.runtime.max_artifact_size_bytes, 64 * 1024 * 1024);
        assert_eq!(config.runtime.runner_ready_timeout_secs, 3);
        assert_eq!(config.runtime.runner_log_buffer_bytes, 256 * 1024);

        cleanup_temp_path(path);
    }

    #[test]
    fn uses_explicit_storage_root_when_present() {
        let path = write_temp_config(
            "uses_explicit_storage_root_when_present",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago-explicit"
server_version = "imagod/test"

[tls]
server_cert = "server.crt"
server_key = "server.key"
client_ca_cert = "ca.crt"
"#,
        );

        let config = ImagodConfig::load(&path).expect("config should load");
        assert_eq!(config.storage_root, PathBuf::from("/tmp/imago-explicit"));

        cleanup_temp_path(path);
    }

    #[test]
    fn resolve_default_storage_root_prefers_build_override() {
        let resolved = resolve_default_storage_root("linux", Some("/tmp/imago-build-default"));
        assert_eq!(resolved, PathBuf::from("/tmp/imago-build-default"));
    }

    #[test]
    fn resolve_default_storage_root_ignores_empty_build_override() {
        let resolved = resolve_default_storage_root("linux", Some(""));
        assert_eq!(resolved, PathBuf::from("/var/lib/imago"));
    }

    #[test]
    fn resolve_default_storage_root_uses_os_matrix() {
        assert_eq!(
            resolve_default_storage_root("linux", None),
            PathBuf::from("/var/lib/imago")
        );
        assert_eq!(
            resolve_default_storage_root("macos", None),
            PathBuf::from("/usr/local/var/imago")
        );
        assert_eq!(
            resolve_default_storage_root("windows", None),
            PathBuf::from(r"C:\ProgramData\imago")
        );
        assert_eq!(
            resolve_default_storage_root("freebsd", None),
            PathBuf::from("/var/lib/imago")
        );
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
    fn rejects_zero_runner_ready_timeout() {
        let path = write_temp_config(
            "rejects_zero_runner_ready_timeout",
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
runner_ready_timeout_secs = 0
"#,
        );

        let err =
            ImagodConfig::load(&path).expect_err("config should reject zero runner_ready_timeout");
        assert!(
            err.to_string()
                .contains("runtime.runner_ready_timeout_secs")
        );

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_runner_log_buffer_bytes() {
        let path = write_temp_config(
            "rejects_zero_runner_log_buffer_bytes",
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
runner_log_buffer_bytes = 0
"#,
        );

        let err = ImagodConfig::load(&path)
            .expect_err("config should reject zero runner_log_buffer_bytes");
        assert!(err.to_string().contains("runtime.runner_log_buffer_bytes"));

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
