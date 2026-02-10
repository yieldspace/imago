use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ImagodError;
use imago_protocol::ErrorCode;

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
    #[serde(default = "default_protocol_draft")]
    pub protocol_draft: String,
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
    #[serde(default = "default_upload_session_ttl_secs")]
    pub upload_session_ttl_secs: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            chunk_size: default_chunk_size(),
            max_inflight_chunks: default_max_inflight_chunks(),
            upload_session_ttl_secs: default_upload_session_ttl_secs(),
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
            .with_detail(
                "path",
                serde_json::Value::String(path.to_string_lossy().to_string()),
            )
        })?;
        let config: Self = toml::from_str(&content).map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "config.load",
                format!("config parse failed: {e}"),
            )
            .with_detail(
                "path",
                serde_json::Value::String(path.to_string_lossy().to_string()),
            )
        })?;
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

fn default_protocol_draft() -> String {
    "imago-mvp-v1".to_string()
}

fn default_chunk_size() -> usize {
    1024 * 1024
}

fn default_max_inflight_chunks() -> usize {
    16
}

fn default_upload_session_ttl_secs() -> u64 {
    15 * 60
}
