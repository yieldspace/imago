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

#[cfg(test)]
mod tests {
    use super::{validate_app_type, validate_dependency_package_name, validate_service_name};

    #[test]
    fn validate_service_name_rejects_empty() {
        let err = validate_service_name("").expect_err("empty name must fail");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn validate_service_name_rejects_too_long_name() {
        let name = "a".repeat(64);
        let err = validate_service_name(&name).expect_err("too long name must fail");
        assert!(err.to_string().contains("63 characters"));
    }

    #[test]
    fn validate_service_name_rejects_path_separator() {
        let err = validate_service_name("svc/name").expect_err("path separator must fail");
        assert!(err.to_string().contains("invalid path characters"));
    }

    #[test]
    fn validate_app_type_accepts_known_values_and_rejects_unknown() {
        for app_type in ["cli", "http", "socket", "rpc"] {
            validate_app_type(app_type).expect("known app type should pass");
        }

        let err = validate_app_type("worker").expect_err("unknown app type must fail");
        assert!(err.to_string().contains("type must be one of"));
    }

    #[test]
    fn validate_dependency_package_name_rejects_parent_traversal() {
        let err = validate_dependency_package_name("acme:pkg/../evil")
            .expect_err("parent traversal must fail");
        assert!(err.to_string().contains("invalid path characters"));
    }

    #[test]
    fn validate_dependency_package_name_rejects_backslash() {
        let err =
            validate_dependency_package_name("acme:pkg\\nested").expect_err("backslash must fail");
        assert!(err.to_string().contains("invalid path characters"));
    }
}
