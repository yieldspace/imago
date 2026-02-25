//! Validation helpers used by lockfile resolution paths.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, anyhow};

pub trait PathVerifier {
    /// Validates a lockfile path field and returns normalized relative path.
    fn validate_safe_wit_path(&self, path: &str, field_name: &str) -> anyhow::Result<PathBuf>;

    /// Ensures no symlink traversal is present on the resolved relative path.
    fn ensure_no_symlink_in_relative_path(
        &self,
        project_root: &Path,
        relative: &Path,
        field_name: &str,
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StrictPathVerifier;

impl PathVerifier for StrictPathVerifier {
    fn validate_safe_wit_path(&self, path: &str, field_name: &str) -> anyhow::Result<PathBuf> {
        validate_safe_wit_path(path, field_name)
    }

    fn ensure_no_symlink_in_relative_path(
        &self,
        project_root: &Path,
        relative: &Path,
        field_name: &str,
    ) -> anyhow::Result<()> {
        ensure_no_symlink_in_relative_path(project_root, relative, field_name)
    }
}

pub(crate) fn validate_wit_source(source: &str, field_name: &str) -> anyhow::Result<()> {
    if source.starts_with("file://")
        || source.starts_with("warg://")
        || source.starts_with("oci://")
    {
        return Ok(());
    }
    if source.starts_with("https://wa.dev/") {
        return Err(anyhow!(
            "{field_name} no longer accepts https://wa.dev shorthand; use warg://<package>@<version>"
        ));
    }
    Err(anyhow!(
        "{field_name} must start with one of: file://, warg://, oci://"
    ))
}

pub(crate) fn validate_sha256_hex(value: &str, field_name: &str) -> anyhow::Result<()> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("{field_name} must be a 64-character hex string"));
    }
    Ok(())
}

pub(crate) fn parse_prefixed_sha256<'a>(
    value: &'a str,
    field_name: &str,
) -> anyhow::Result<&'a str> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(anyhow!("{field_name} must start with 'sha256:'"));
    };
    validate_sha256_hex(hex, field_name)?;
    Ok(hex)
}

pub(crate) fn validate_safe_wit_path(path: &str, field_name: &str) -> anyhow::Result<PathBuf> {
    if path.trim().is_empty() {
        return Err(anyhow!(
            "{field_name} must not be empty; run `imago update`"
        ));
    }
    let raw = Path::new(path);
    if raw.is_absolute() {
        return Err(anyhow!(
            "{field_name} must be a relative path under wit/deps; run `imago update`"
        ));
    }

    let mut normalized = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            _ => {
                return Err(anyhow!(
                    "{field_name} contains invalid path components; run `imago update`"
                ));
            }
        }
    }

    let mut components = normalized.components();
    let first = components.next();
    let second = components.next();
    if !matches!(first, Some(Component::Normal(v)) if v == "wit")
        || !matches!(second, Some(Component::Normal(v)) if v == "deps")
    {
        return Err(anyhow!(
            "{field_name} must be under wit/deps; run `imago update`"
        ));
    }

    Ok(normalized)
}

pub(crate) fn ensure_no_symlink_in_relative_path(
    project_root: &Path,
    relative: &Path,
    field_name: &str,
) -> anyhow::Result<()> {
    let mut current = project_root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(anyhow!(
                "{field_name} contains invalid path components; run `imago update`"
            ));
        };
        current.push(part);
        if !current.exists() {
            break;
        }
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("failed to inspect path {}", current.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(anyhow!(
                "{field_name} resolves through symlink '{}'; run `imago update`",
                current.display()
            ));
        }
    }
    Ok(())
}
