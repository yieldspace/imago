//! Configuration loader and validation for the `imagod` manager process.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use imago_protocol::ErrorCode;
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
/// TLS material locations for raw public key mTLS server startup.
pub struct TlsConfig {
    /// Server private key in PEM format.
    pub server_key: PathBuf,
    #[serde(default)]
    /// Allowlist of admin Ed25519 raw public keys (32-byte hex).
    pub admin_public_keys: Vec<String>,
    /// Allowlist of client Ed25519 raw public keys (32-byte hex).
    pub client_public_keys: Vec<String>,
    #[serde(default)]
    /// TOFU-known remote authorities mapped to Ed25519 raw public key hex.
    pub known_public_keys: BTreeMap<String, String>,
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
    #[serde(default = "default_http_worker_count")]
    /// Number of HTTP worker tasks used by runtime ingress.
    pub http_worker_count: u32,
    #[serde(default = "default_http_worker_queue_capacity")]
    /// Per-worker request queue capacity for runtime ingress.
    pub http_worker_queue_capacity: u32,
    #[serde(default = "default_manager_control_read_timeout_ms")]
    /// Read timeout for manager control channel operations in milliseconds.
    pub manager_control_read_timeout_ms: u64,
    #[serde(default = "default_max_concurrent_sessions")]
    /// Upper bound of concurrently active sessions managed by imagod.
    pub max_concurrent_sessions: u32,
    #[serde(default = "default_deploy_stream_timeout_secs")]
    /// Timeout for deployment stream operations in seconds.
    pub deploy_stream_timeout_secs: u64,
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
            http_worker_count: default_http_worker_count(),
            http_worker_queue_capacity: default_http_worker_queue_capacity(),
            manager_control_read_timeout_ms: default_manager_control_read_timeout_ms(),
            max_concurrent_sessions: default_max_concurrent_sessions(),
            deploy_stream_timeout_secs: default_deploy_stream_timeout_secs(),
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

/// Adds one public key to `tls.client_public_keys` and updates
/// `tls.known_public_keys[authority]` in `imagod.toml` atomically.
pub fn upsert_tls_known_client_key(
    config_path: &Path,
    authority: &str,
    public_key_hex: &str,
) -> Result<(), ImagodError> {
    let authority = authority.trim();
    if authority.is_empty() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.update",
            "authority must not be empty",
        ));
    }
    parse_ed25519_raw_public_key_hex(public_key_hex).map_err(|reason| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.update",
            format!("public key is invalid: {reason}"),
        )
    })?;

    let mut root = load_toml_root(config_path)?;
    let tls = ensure_tls_table_mut(&mut root)?;

    let key_lower = public_key_hex.to_ascii_lowercase();
    upsert_client_public_key_value(tls, &key_lower)?;
    upsert_known_public_key_value(tls, authority, &key_lower)?;

    write_toml_root_atomic(config_path, &root)?;
    let _ = ImagodConfig::load(config_path)?;
    Ok(())
}

/// Adds or updates one `tls.known_public_keys[authority]` entry in
/// `imagod.toml` atomically.
pub fn upsert_tls_known_public_key(
    config_path: &Path,
    authority: &str,
    public_key_hex: &str,
) -> Result<(), ImagodError> {
    let authority = authority.trim();
    if authority.is_empty() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            "config.update",
            "authority must not be empty",
        ));
    }
    parse_ed25519_raw_public_key_hex(public_key_hex).map_err(|reason| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.update",
            format!("public key is invalid: {reason}"),
        )
    })?;

    let mut root = load_toml_root(config_path)?;
    let tls = ensure_tls_table_mut(&mut root)?;

    let key_lower = public_key_hex.to_ascii_lowercase();
    upsert_known_public_key_value(tls, authority, &key_lower)?;

    write_toml_root_atomic(config_path, &root)?;
    let _ = ImagodConfig::load(config_path)?;
    Ok(())
}

fn load_toml_root(config_path: &Path) -> Result<toml::Table, ImagodError> {
    let body = fs::read_to_string(config_path).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!("failed to read {}: {e}", config_path.display()),
        )
    })?;
    let parsed: toml::Value = toml::from_str(&body).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.update",
            format!("failed to parse {}: {e}", config_path.display()),
        )
    })?;
    parsed.as_table().cloned().ok_or_else(|| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.update",
            "root must be a table",
        )
    })
}

fn ensure_tls_table_mut(root: &mut toml::Table) -> Result<&mut toml::Table, ImagodError> {
    if !root.contains_key("tls") {
        root.insert("tls".to_string(), toml::Value::Table(toml::Table::new()));
    }
    root.get_mut("tls")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.update",
                "tls must be a table",
            )
        })
}

fn upsert_client_public_key_value(tls: &mut toml::Table, key_hex: &str) -> Result<(), ImagodError> {
    if !tls.contains_key("client_public_keys") {
        tls.insert(
            "client_public_keys".to_string(),
            toml::Value::Array(Vec::new()),
        );
    }
    let array = tls
        .get_mut("client_public_keys")
        .and_then(toml::Value::as_array_mut)
        .ok_or_else(|| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.update",
                "tls.client_public_keys must be an array",
            )
        })?;

    let mut normalized = Vec::with_capacity(array.len() + 1);
    for (index, value) in array.iter().enumerate() {
        let text = value.as_str().ok_or_else(|| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.update",
                format!("tls.client_public_keys[{index}] must be a string"),
            )
        })?;
        normalized.push(text.to_ascii_lowercase());
    }
    if !normalized.iter().any(|existing| existing == key_hex) {
        normalized.push(key_hex.to_string());
    }
    *array = normalized
        .into_iter()
        .map(toml::Value::String)
        .collect::<Vec<_>>();
    Ok(())
}

fn upsert_known_public_key_value(
    tls: &mut toml::Table,
    authority: &str,
    key_hex: &str,
) -> Result<(), ImagodError> {
    if !tls.contains_key("known_public_keys") {
        tls.insert(
            "known_public_keys".to_string(),
            toml::Value::Table(toml::Table::new()),
        );
    }
    let map = tls
        .get_mut("known_public_keys")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.update",
                "tls.known_public_keys must be a table",
            )
        })?;

    for (existing_authority, value) in map.iter() {
        if value.as_str().is_none() {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                "config.update",
                format!("tls.known_public_keys['{existing_authority}'] must be a string"),
            ));
        }
    }
    map.insert(
        authority.to_string(),
        toml::Value::String(key_hex.to_string()),
    );
    Ok(())
}

fn write_toml_root_atomic(config_path: &Path, root: &toml::Table) -> Result<(), ImagodError> {
    let body = toml::to_string_pretty(root).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!("failed to serialize config: {e}"),
        )
    })?;

    let parent = config_path.parent().ok_or_else(|| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!("config path has no parent: {}", config_path.display()),
        )
    })?;
    fs::create_dir_all(parent).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!("failed to create parent dir {}: {e}", parent.display()),
        )
    })?;

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = parent.join(format!(
        ".{}.tmp-{unique}",
        config_path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("imagod.toml")
    ));

    let mut file = fs::File::create(&tmp_path).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!("failed to create temp file {}: {e}", tmp_path.display()),
        )
    })?;
    file.write_all(body.as_bytes()).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!("failed to write temp file {}: {e}", tmp_path.display()),
        )
    })?;
    file.sync_all().map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!("failed to flush temp file {}: {e}", tmp_path.display()),
        )
    })?;

    fs::rename(&tmp_path, config_path).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            "config.update",
            format!(
                "failed to atomically replace {} with {}: {e}",
                config_path.display(),
                tmp_path.display()
            ),
        )
    })?;
    Ok(())
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

fn default_http_worker_count() -> u32 {
    2
}

fn default_http_worker_queue_capacity() -> u32 {
    4
}

fn default_manager_control_read_timeout_ms() -> u64 {
    500
}

fn default_max_concurrent_sessions() -> u32 {
    256
}

fn default_deploy_stream_timeout_secs() -> u64 {
    15
}

/// Parse a 32-byte Ed25519 raw public key from hex.
pub fn parse_ed25519_raw_public_key_hex(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err(format!("must be 64 hex characters (got {})", value.len()));
    }

    let bytes = value.as_bytes();
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let pos = index * 2;
        let hi = decode_hex_nibble(bytes[pos]).ok_or_else(|| {
            format!("must contain only hex characters (invalid byte at position {pos})")
        })?;
        let lo = decode_hex_nibble(bytes[pos + 1]).ok_or_else(|| {
            format!(
                "must contain only hex characters (invalid byte at position {})",
                pos + 1
            )
        })?;
        *slot = (hi << 4) | lo;
    }

    Ok(out)
}

fn decode_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
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
        assert_eq!(config.runtime.http_worker_count, 2);
        assert_eq!(config.runtime.http_worker_queue_capacity, 4);
        assert_eq!(config.runtime.manager_control_read_timeout_ms, 500);
        assert_eq!(config.runtime.max_concurrent_sessions, 256);
        assert_eq!(config.runtime.deploy_stream_timeout_secs, 15);

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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject legacy key");
        let message = err.to_string();
        assert!(message.contains("protocol_draft"));
        assert!(message.contains("compatibility_date"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_legacy_tls_server_cert_key() {
        let path = write_temp_config(
            "rejects_legacy_tls_server_cert_key",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
server_cert = "server.crt"
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject tls.server_cert");
        assert!(err.to_string().contains("tls.server_cert"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_legacy_tls_client_ca_cert_key() {
        let path = write_temp_config(
            "rejects_legacy_tls_client_ca_cert_key",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
client_ca_cert = "ca.crt"
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject tls.client_ca_cert");
        assert!(err.to_string().contains("tls.client_ca_cert"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_empty_client_public_keys() {
        let path = write_temp_config(
            "rejects_empty_client_public_keys",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = []
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject empty allowlist");
        assert!(err.to_string().contains("tls.client_public_keys"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_duplicate_client_public_keys() {
        let path = write_temp_config(
            "rejects_duplicate_client_public_keys",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = [
  "1111111111111111111111111111111111111111111111111111111111111111",
  "1111111111111111111111111111111111111111111111111111111111111111",
]
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject duplicated key");
        assert!(err.to_string().contains("duplicated"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_non_hex_client_public_key() {
        let path = write_temp_config(
            "rejects_non_hex_client_public_key",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["zz11111111111111111111111111111111111111111111111111111111111111"]
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject non-hex key");
        assert!(err.to_string().contains("hex"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_invalid_length_client_public_key() {
        let path = write_temp_config(
            "rejects_invalid_length_client_public_key",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["11111111111111111111111111111111111111111111111111111111111111"]
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject invalid length key");
        assert!(err.to_string().contains("64 hex"));

        cleanup_temp_path(path);
    }

    #[test]
    fn accepts_admin_and_known_public_keys() {
        let path = write_temp_config(
            "accepts_admin_and_known_public_keys",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
admin_public_keys = ["2222222222222222222222222222222222222222222222222222222222222222"]
known_public_keys = { "rpc://node-a:4443" = "1111111111111111111111111111111111111111111111111111111111111111", "rpc://node-b:4443" = "2222222222222222222222222222222222222222222222222222222222222222" }
"#,
        );

        let config = ImagodConfig::load(&path).expect("config should load");
        assert_eq!(config.tls.admin_public_keys.len(), 1);
        assert_eq!(config.tls.known_public_keys.len(), 2);

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_duplicate_admin_public_keys() {
        let path = write_temp_config(
            "rejects_duplicate_admin_public_keys",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
admin_public_keys = [
  "2222222222222222222222222222222222222222222222222222222222222222",
  "2222222222222222222222222222222222222222222222222222222222222222",
]
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject duplicated admin key");
        assert!(err.to_string().contains("tls.admin_public_keys"));
        assert!(err.to_string().contains("duplicated"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_admin_client_key_overlap() {
        let path = write_temp_config(
            "rejects_admin_client_key_overlap",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
admin_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject overlap");
        assert!(err.to_string().contains("overlaps"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_known_public_keys_with_empty_authority() {
        let path = write_temp_config(
            "rejects_known_public_keys_with_empty_authority",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
admin_public_keys = ["2222222222222222222222222222222222222222222222222222222222222222"]
known_public_keys = { "" = "2222222222222222222222222222222222222222222222222222222222222222" }
"#,
        );

        let err = ImagodConfig::load(&path).expect_err("config should reject empty authority");
        assert!(err.to_string().contains("authority"));
        assert!(err.to_string().contains("tls.known_public_keys"));

        cleanup_temp_path(path);
    }

    #[test]
    fn parses_ed25519_raw_public_key_hex() {
        let key = parse_ed25519_raw_public_key_hex(
            "00112233445566778899AABBCCDDEEFF00112233445566778899aabbccddeeff",
        )
        .expect("hex parsing should succeed");
        assert_eq!(
            key,
            [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb,
                0xcc, 0xdd, 0xee, 0xff
            ]
        );
    }

    #[test]
    fn rejects_invalid_ed25519_raw_public_key_hex() {
        let err = parse_ed25519_raw_public_key_hex("00xx")
            .expect_err("hex parser should reject invalid length and chars");
        assert!(err.contains("64 hex"));
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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

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
    fn rejects_http_worker_count_out_of_range() {
        let path = write_temp_config(
            "rejects_http_worker_count_out_of_range",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

[runtime]
http_worker_count = 5
"#,
        );

        let err =
            ImagodConfig::load(&path).expect_err("config should reject out-of-range worker count");
        assert!(err.to_string().contains("runtime.http_worker_count"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_http_worker_queue_capacity_out_of_range() {
        let path = write_temp_config(
            "rejects_http_worker_queue_capacity_out_of_range",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

[runtime]
http_worker_queue_capacity = 17
"#,
        );

        let err = ImagodConfig::load(&path)
            .expect_err("config should reject out-of-range queue capacity");
        assert!(
            err.to_string()
                .contains("runtime.http_worker_queue_capacity")
        );

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_manager_control_read_timeout_ms() {
        let path = write_temp_config(
            "rejects_zero_manager_control_read_timeout_ms",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

[runtime]
manager_control_read_timeout_ms = 0
"#,
        );

        let err = ImagodConfig::load(&path)
            .expect_err("config should reject zero manager control read timeout");
        assert!(
            err.to_string()
                .contains("runtime.manager_control_read_timeout_ms")
        );

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_max_concurrent_sessions() {
        let path = write_temp_config(
            "rejects_zero_max_concurrent_sessions",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

[runtime]
max_concurrent_sessions = 0
"#,
        );

        let err = ImagodConfig::load(&path)
            .expect_err("config should reject zero max_concurrent_sessions");
        assert!(err.to_string().contains("runtime.max_concurrent_sessions"));

        cleanup_temp_path(path);
    }

    #[test]
    fn rejects_zero_deploy_stream_timeout_secs() {
        let path = write_temp_config(
            "rejects_zero_deploy_stream_timeout_secs",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

[runtime]
deploy_stream_timeout_secs = 0
"#,
        );

        let err = ImagodConfig::load(&path)
            .expect_err("config should reject zero deploy_stream_timeout_secs");
        assert!(
            err.to_string()
                .contains("runtime.deploy_stream_timeout_secs")
        );

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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

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
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]

[runtime]
chunk_size = 8388609
"#,
        );

        let err =
            ImagodConfig::load(&path).expect_err("config should reject chunk_size above 8MiB");
        assert!(err.to_string().contains("runtime.chunk_size"));

        cleanup_temp_path(path);
    }

    #[test]
    fn upsert_known_public_key_updates_known_map_only() {
        let path = write_temp_config(
            "upsert_known_public_key_updates_known_map_only",
            r#"
listen_addr = "127.0.0.1:4443"
storage_root = "/tmp/imago"
server_version = "imagod/test"
compatibility_date = "2026-02-10"

[tls]
server_key = "server.key"
client_public_keys = ["1111111111111111111111111111111111111111111111111111111111111111"]
known_public_keys = {}

[runtime]
chunk_size = 1048576
max_inflight_chunks = 16
max_artifact_size_bytes = 67108864
upload_session_ttl_secs = 600
stop_grace_timeout_secs = 30
runner_ready_timeout_secs = 3
runner_log_buffer_bytes = 262144
epoch_tick_interval_ms = 50
http_worker_count = 2
http_worker_queue_capacity = 4
manager_control_read_timeout_ms = 500
max_concurrent_sessions = 256
deploy_stream_timeout_secs = 15
"#,
        );

        upsert_tls_known_public_key(
            &path,
            "rpc://node-a:4443",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("known key update should succeed");

        let config = ImagodConfig::load(&path).expect("config should load");
        assert_eq!(config.tls.client_public_keys.len(), 1);
        assert_eq!(
            config
                .tls
                .known_public_keys
                .get("rpc://node-a:4443")
                .cloned()
                .expect("known key entry should be created"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

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
