//! Validation helpers for `imago.toml` build-time constraints.
//!
//! These checks keep path and identifier handling deterministic across
//! build/update/deploy flows and reject traversal or unsupported symbols early.

use std::path::{Component, Path};

use anyhow::anyhow;

/// Validates service names used in config and manifest records.
pub(crate) fn validate_service_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        return Err(anyhow!("name must not be empty"));
    }
    if name.len() > 63 {
        return Err(anyhow!("name must be 63 characters or fewer"));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(anyhow!("name contains invalid path characters: {}", name));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(anyhow!("name contains unsupported characters: {}", name));
    }
    Ok(())
}

/// Validates declared app execution type.
pub(crate) fn validate_app_type(app_type: &str) -> anyhow::Result<()> {
    match app_type {
        "cli" | "http" | "socket" | "rpc" => Ok(()),
        _ => Err(anyhow!(
            "type must be one of: cli, http, socket, rpc (got: {})",
            app_type
        )),
    }
}

/// Validates dependency package names including nested namespace segments.
pub(crate) fn validate_dependency_package_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        return Err(anyhow!("must not be empty"));
    }
    if name.contains('\\') || name.contains("..") {
        return Err(anyhow!("contains invalid path characters: {name}"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
    {
        return Err(anyhow!("contains unsupported characters: {name}"));
    }
    if name
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(anyhow!("contains invalid path components: {name}"));
    }
    for component in Path::new(name).components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(anyhow!("contains invalid path components: {name}"));
        }
    }
    Ok(())
}
