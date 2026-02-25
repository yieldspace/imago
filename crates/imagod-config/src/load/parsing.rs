//! TOML parsing and typed decode helpers for `imagod.toml`.

use std::path::Path;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;

use crate::ImagodConfig;

pub(crate) fn parse(path: &Path, content: &str) -> Result<toml::Value, ImagodError> {
    toml::from_str(content).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!("config parse failed: {e}"),
        )
        .with_detail("path", path.to_string_lossy())
    })
}

pub(crate) fn decode(path: &Path, raw: toml::Value) -> Result<ImagodConfig, ImagodError> {
    raw.try_into().map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!("config decode failed: {e}"),
        )
        .with_detail("path", path.to_string_lossy())
    })
}
