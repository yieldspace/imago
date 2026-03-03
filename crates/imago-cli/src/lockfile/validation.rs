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
    let source = source.trim();
    if source.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    if source.contains('\n') || source.contains('\r') {
        return Err(anyhow!("{field_name} must not contain newline"));
    }
    if source.starts_with("warg://") || source.starts_with("oci://") {
        return Err(anyhow!(
            "{field_name} must not use warg:// or oci:// prefixes"
        ));
    }
    if source.starts_with("https://wa.dev/") || source.starts_with("http://wa.dev/") {
        return Err(anyhow!(
            "{field_name} no longer accepts wa.dev shorthand URL"
        ));
    }
    if let Some((scheme, _rest)) = source.split_once("://")
        && !matches!(scheme, "http" | "https" | "file")
    {
        return Err(anyhow!(
            "{field_name} URL scheme '{scheme}' is not supported; use file/http/https or plain source"
        ));
    }
    Ok(())
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
            "{field_name} must not be empty; run `imago deps sync`"
        ));
    }
    let raw = Path::new(path);
    if raw.is_absolute() {
        return Err(anyhow!(
            "{field_name} must be a relative path under wit/deps; run `imago deps sync`"
        ));
    }

    let mut normalized = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            _ => {
                return Err(anyhow!(
                    "{field_name} contains invalid path components; run `imago deps sync`"
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
            "{field_name} must be under wit/deps; run `imago deps sync`"
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
                "{field_name} contains invalid path components; run `imago deps sync`"
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
                "{field_name} resolves through symlink '{}'; run `imago deps sync`",
                current.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{
        ensure_no_symlink_in_relative_path, parse_prefixed_sha256, validate_safe_wit_path,
        validate_wit_source,
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-lockfile-validation-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    #[test]
    fn validate_safe_wit_path_accepts_wit_deps_path() {
        let normalized = validate_safe_wit_path("wit/deps/acme-test-0.1.0", "field")
            .expect("wit/deps path should pass");
        assert_eq!(normalized, PathBuf::from("wit/deps/acme-test-0.1.0"));
    }

    #[test]
    fn validate_safe_wit_path_rejects_absolute_path() {
        let err = validate_safe_wit_path("/tmp/wit/deps/pkg", "field")
            .expect_err("absolute path must fail");
        assert!(err.to_string().contains("relative path"));
    }

    #[test]
    fn validate_safe_wit_path_rejects_path_outside_wit_deps() {
        let err = validate_safe_wit_path("wit/cache/pkg", "field")
            .expect_err("non wit/deps path must fail");
        assert!(err.to_string().contains("must be under wit/deps"));
    }

    #[test]
    fn validate_safe_wit_path_rejects_parent_traversal() {
        let err = validate_safe_wit_path("wit/deps/../pkg", "field")
            .expect_err("parent traversal must fail");
        assert!(err.to_string().contains("invalid path components"));
    }

    #[test]
    fn parse_prefixed_sha256_rejects_missing_prefix() {
        let err = parse_prefixed_sha256("abc", "field").expect_err("missing prefix must fail");
        assert!(err.to_string().contains("must start with 'sha256:'"));
    }

    #[test]
    fn parse_prefixed_sha256_rejects_invalid_hex_length() {
        let err = parse_prefixed_sha256("sha256:abc", "field").expect_err("short digest must fail");
        assert!(err.to_string().contains("64-character hex"));
    }

    #[test]
    fn validate_wit_source_rejects_unsupported_scheme() {
        let err = validate_wit_source("ftp://example.com/pkg", "field")
            .expect_err("unsupported scheme must fail");
        assert!(err.to_string().contains("not supported"));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_no_symlink_in_relative_path_rejects_symlink_component() {
        use std::os::unix::fs::symlink;

        let root = new_temp_dir("symlink-reject");
        fs::create_dir_all(root.join("wit/deps")).expect("wit/deps should exist");
        fs::write(root.join("target-dir"), b"x").expect("target should exist");
        symlink(root.join("target-dir"), root.join("wit/deps/link"))
            .expect("symlink should be created");

        let err = ensure_no_symlink_in_relative_path(
            &root,
            PathBuf::from("wit/deps/link").as_path(),
            "field",
        )
        .expect_err("symlink should be rejected");
        assert!(err.to_string().contains("resolves through symlink"));
    }
}
