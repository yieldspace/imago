use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};

use crate::commands::plugin_sources;

use super::{
    CACHE_ROOT_REL, MISSING_CACHE_HINT,
    digest::{compute_path_digest_hex, compute_sha256_hex},
    model::{DependencyCacheEntry, parse_prefixed_sha256},
};

pub(crate) fn cache_entry_root(project_root: &Path, dependency_name: &str) -> PathBuf {
    project_root
        .join(CACHE_ROOT_REL)
        .join(plugin_sources::sanitize_wit_deps_name(dependency_name))
}

pub(crate) fn cache_component_path(
    project_root: &Path,
    dependency_name: &str,
    sha256: &str,
) -> PathBuf {
    cache_entry_root(project_root, dependency_name)
        .join("components")
        .join(format!("{sha256}.wasm"))
}

pub(crate) fn load_entry(
    project_root: &Path,
    dependency_name: &str,
) -> anyhow::Result<DependencyCacheEntry> {
    let meta_path = meta_path(project_root, dependency_name);
    let raw = fs::read_to_string(&meta_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!(
                "dependency '{}' cache is missing under {}; {}",
                dependency_name,
                CACHE_ROOT_REL,
                MISSING_CACHE_HINT
            )
        } else {
            anyhow!(
                "failed to read dependency cache metadata {}: {err}",
                meta_path.display()
            )
        }
    })?;
    let entry: DependencyCacheEntry = toml::from_str(&raw).with_context(|| {
        format!(
            "failed to parse dependency cache metadata {}",
            meta_path.display()
        )
    })?;
    if entry.name != dependency_name {
        return Err(anyhow!(
            "dependency '{}' cache metadata name mismatch (meta='{}'); {}",
            dependency_name,
            entry.name,
            MISSING_CACHE_HINT
        ));
    }
    Ok(entry)
}

pub(crate) fn save_entry(project_root: &Path, entry: &DependencyCacheEntry) -> anyhow::Result<()> {
    let path = meta_path(project_root, &entry.name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create dependency cache dir {}", parent.display())
        })?;
    }
    let body =
        toml::to_string_pretty(entry).context("failed to serialize dependency cache metadata")?;
    fs::write(&path, body).with_context(|| {
        format!(
            "failed to write dependency cache metadata {}",
            path.display()
        )
    })?;
    Ok(())
}

pub(super) fn meta_path(project_root: &Path, dependency_name: &str) -> PathBuf {
    cache_entry_root(project_root, dependency_name).join("meta.toml")
}

pub(super) fn resolve_existing_file_source_path(
    project_root: &Path,
    source: &str,
    source_kind: plugin_sources::SourceKind,
) -> anyhow::Result<Option<PathBuf>> {
    if source_kind != plugin_sources::SourceKind::Path {
        return Ok(None);
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        return Ok(None);
    }

    let raw_path = source.strip_prefix("file://").unwrap_or(source);
    if raw_path.trim().is_empty() {
        return Err(anyhow!("path source must not be empty"));
    }
    let path = PathBuf::from(raw_path);
    let resolved = if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    };
    let metadata = match fs::symlink_metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(anyhow!(
                "failed to inspect path source {}: {err}",
                resolved.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "symlink paths are not allowed while resolving path source: {}",
            resolved.display()
        ));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(anyhow!(
            "resolved path source is not a file or directory: {}",
            resolved.display()
        ));
    }
    Ok(Some(resolved))
}

pub(super) fn entry_files_are_complete(
    project_root: &Path,
    entry: &DependencyCacheEntry,
) -> anyhow::Result<bool> {
    let entry_root = cache_entry_root(project_root, &entry.name);
    let direct_wit_path = entry_root.join(&entry.wit_path);
    if !direct_wit_path.exists() {
        return Ok(false);
    }
    if compute_path_digest_hex(&direct_wit_path)? != entry.wit_digest {
        return Ok(false);
    }

    for transitive in &entry.transitive_packages {
        let expected_digest = parse_prefixed_sha256(&transitive.digest, "transitive digest")?;
        let file_path = entry_root.join(&transitive.path).join("package.wit");
        if !file_path.is_file() {
            return Ok(false);
        }
        let actual_digest = compute_sha256_hex(&file_path)?;
        if !actual_digest.eq_ignore_ascii_case(expected_digest) {
            return Ok(false);
        }
    }

    if entry.kind == "wasm" {
        let component_sha = match entry.component_sha256.as_ref() {
            Some(value) => value,
            None => return Ok(false),
        };
        let component_path = cache_component_path(project_root, &entry.name, component_sha);
        if !component_path.is_file() {
            return Ok(false);
        }
        if !compute_sha256_hex(&component_path)?.eq_ignore_ascii_case(component_sha) {
            return Ok(false);
        }
    }

    Ok(true)
}

pub(super) fn copy_tree_with_conflict_check(
    source: &Path,
    destination: &Path,
) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect source path {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "symlink paths are not allowed while copying cache files: {}",
            source.display()
        ));
    }
    if metadata.is_file() {
        copy_file_with_conflict_check(source, destination)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(anyhow!(
            "source path is not file or dir: {}",
            source.display()
        ));
    }
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create destination {}", destination.display()))?;

    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read source directory {}", source.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to read source directory entry in {}",
                source.display()
            )
        })?;
        let source_path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect source path {}", source_path.display()))?;
        if file_type.is_symlink() {
            return Err(anyhow!(
                "symlink paths are not allowed while copying cache files: {}",
                source_path.display()
            ));
        }
        let file_name = source_path.file_name().ok_or_else(|| {
            anyhow!(
                "failed to resolve source file name under {}",
                source.display()
            )
        })?;
        let destination_path = destination.join(file_name);
        if file_type.is_dir() {
            copy_tree_with_conflict_check(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            copy_file_with_conflict_check(&source_path, &destination_path)?;
        } else {
            return Err(anyhow!(
                "source path is not file or dir: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn copy_file_with_conflict_check(
    source: &Path,
    destination: &Path,
) -> anyhow::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create destination parent {}", parent.display()))?;
    }

    if destination.exists() {
        let source_digest = compute_sha256_hex(source)?;
        let destination_digest = compute_sha256_hex(destination)?;
        if source_digest != destination_digest {
            return Err(anyhow!(
                "conflicting cached WIT package detected at {}",
                destination.display()
            ));
        }
        return Ok(());
    }

    fs::copy(source, destination).with_context(|| {
        format!(
            "failed to copy cached file {} -> {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::commands::plugin_sources;

    use super::{
        DependencyCacheEntry, cache_entry_root, copy_tree_with_conflict_check,
        entry_files_are_complete, load_entry, resolve_existing_file_source_path, save_entry,
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-dependency-cache-io-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn sample_entry() -> DependencyCacheEntry {
        DependencyCacheEntry {
            name: "path-source-0".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "native".to_string(),
            wit_source: "registry/example".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: "wit/deps/path-source-0-0.1.0".to_string(),
            wit_digest: String::new(),
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        }
    }

    #[test]
    fn save_entry_and_load_entry_roundtrip() {
        let root = new_temp_dir("roundtrip");
        let entry = sample_entry();

        save_entry(&root, &entry).expect("entry should save");
        let loaded = load_entry(&root, &entry.name).expect("entry should load");
        assert_eq!(loaded.name, entry.name);
        assert_eq!(loaded.version, entry.version);
        assert_eq!(loaded.kind, entry.kind);
    }

    #[test]
    fn resolve_existing_file_source_path_handles_http_and_empty_path() {
        let root = new_temp_dir("source-path");
        let http = resolve_existing_file_source_path(
            &root,
            "https://example.com/pkg",
            plugin_sources::SourceKind::Path,
        )
        .expect("http source should be ignored");
        assert!(http.is_none());

        let err = resolve_existing_file_source_path(&root, "", plugin_sources::SourceKind::Path)
            .expect_err("empty path source should fail");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_file_source_path_rejects_symlink_source() {
        use std::os::unix::fs::symlink;

        let root = new_temp_dir("source-symlink");
        fs::write(root.join("real.wit"), b"package demo:test@0.1.0;\n")
            .expect("real file should be written");
        symlink(root.join("real.wit"), root.join("linked.wit")).expect("symlink should be created");

        let err = resolve_existing_file_source_path(
            &root,
            "linked.wit",
            plugin_sources::SourceKind::Path,
        )
        .expect_err("symlink source should be rejected");
        assert!(err.to_string().contains("symlink paths are not allowed"));
    }

    #[test]
    fn entry_files_are_complete_detects_digest_mismatch() {
        let root = new_temp_dir("complete");
        let mut entry = sample_entry();
        let entry_root = cache_entry_root(&root, &entry.name);
        let direct_wit = entry_root.join(&entry.wit_path);
        fs::create_dir_all(&direct_wit).expect("wit dir should be created");
        fs::write(
            direct_wit.join("package.wit"),
            b"package demo:test@0.1.0;\n",
        )
        .expect("package.wit should be written");
        entry.wit_digest = super::compute_path_digest_hex(&direct_wit).expect("digest");

        assert!(
            entry_files_are_complete(&root, &entry).expect("completeness check should succeed")
        );

        fs::write(
            direct_wit.join("package.wit"),
            b"package demo:test@0.2.0;\n",
        )
        .expect("package.wit should be rewritten");
        assert!(
            !entry_files_are_complete(&root, &entry).expect("mismatch should be reported as false")
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_with_conflict_check_rejects_symlink_entry() {
        use std::os::unix::fs::symlink;

        let root = new_temp_dir("copy-symlink");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("source dir should be created");
        fs::write(source.join("file.txt"), b"ok").expect("source file should be written");
        symlink(source.join("file.txt"), source.join("link.txt"))
            .expect("symlink should be created");

        let err = copy_tree_with_conflict_check(&source, &destination)
            .expect_err("symlink entry should be rejected");
        assert!(err.to_string().contains("symlink paths are not allowed"));
    }
}
