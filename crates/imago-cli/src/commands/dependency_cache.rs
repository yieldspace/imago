use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::commands::{
    build::{ManifestDependencyKind, ProjectDependency},
    plugin_sources,
};

const CACHE_ROOT_REL: &str = ".imago/deps";
const MISSING_CACHE_HINT: &str = "run `imago deps sync`";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DependencyCacheEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_package_name: Option<String>,
    pub version: String,
    pub kind: String,
    pub wit_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit_registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit_sha256: Option<String>,
    pub wit_path: String,
    pub wit_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit_source_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_source_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_world_foreign_packages: Vec<DependencyCacheComponentWorldForeignPackage>,
    #[serde(default)]
    pub component_world_foreign_packages_recorded: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitive_packages: Vec<DependencyCacheTransitivePackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DependencyCacheTransitivePackage {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    pub requirement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DependencyCacheComponentWorldForeignPackage {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<String>,
    #[serde(default)]
    pub interfaces_recorded: bool,
}

impl DependencyCacheEntry {
    pub(crate) fn validate_for_dependency(
        &self,
        dependency: &ProjectDependency,
        namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
    ) -> anyhow::Result<()> {
        if self.name != dependency.name {
            return Err(anyhow!(
                "cache name mismatch (cache='{}', config='{}')",
                self.name,
                dependency.name
            ));
        }
        if self.version != dependency.version {
            return Err(anyhow!(
                "cache version mismatch (cache='{}', config='{}')",
                self.version,
                dependency.version
            ));
        }
        if self.kind != dependency_kind_label(dependency.kind) {
            return Err(anyhow!(
                "cache kind mismatch (cache='{}', config='{}')",
                self.kind,
                dependency_kind_label(dependency.kind)
            ));
        }
        if self.wit_source != dependency.wit.source {
            return Err(anyhow!(
                "cache wit source mismatch (cache='{}', config='{}')",
                self.wit_source,
                dependency.wit.source
            ));
        }
        if self.wit_registry != dependency.wit.registry {
            return Err(anyhow!(
                "cache wit registry mismatch (cache='{}', config='{}')",
                self.wit_registry.as_deref().unwrap_or(""),
                dependency.wit.registry.as_deref().unwrap_or("")
            ));
        }
        if self.wit_sha256 != dependency.wit.sha256 {
            return Err(anyhow!(
                "cache wit sha256 mismatch (cache='{}', config='{}')",
                self.wit_sha256.as_deref().unwrap_or(""),
                dependency.wit.sha256.as_deref().unwrap_or("")
            ));
        }
        let expected_wit_path = dependency_wit_path(&dependency.name, &dependency.version);
        if self.wit_path != expected_wit_path {
            return Err(anyhow!(
                "cache wit path mismatch (cache='{}', expected='{}')",
                self.wit_path,
                expected_wit_path
            ));
        }
        validate_safe_wit_path(&self.wit_path, "wit_path")?;

        for transitive in &self.transitive_packages {
            validate_safe_wit_path(&transitive.path, "transitive_packages[].path")?;
            if transitive.requirement.trim().is_empty() {
                return Err(anyhow!(
                    "cache transitive requirement is empty for '{}'",
                    transitive.name
                ));
            }
            parse_prefixed_sha256(&transitive.digest, "transitive_packages[].digest")?;
            if dependency.wit.source_kind == plugin_sources::SourceKind::Wit
                && transitive.version.is_some()
                && transitive.registry.is_some()
            {
                let expected_registry =
                    plugin_sources::resolve_warg_registry_for_package_with_fallback(
                        &transitive.name,
                        None,
                        namespace_registries,
                        dependency.wit.registry.as_deref(),
                    )?;
                if transitive.registry.as_deref() != Some(expected_registry.as_str()) {
                    return Err(anyhow!(
                        "cache transitive registry mismatch for '{}' (cache='{}', expected='{}')",
                        transitive.name,
                        transitive.registry.as_deref().unwrap_or(""),
                        expected_registry
                    ));
                }
            }
        }

        match dependency.kind {
            ManifestDependencyKind::Native => {}
            ManifestDependencyKind::Wasm => {
                if !self.component_world_foreign_packages_recorded {
                    return Err(anyhow!(
                        "cache component world foreign package metadata is missing for wasm dependency"
                    ));
                }
                for package in &self.component_world_foreign_packages {
                    if package.name.trim().is_empty() {
                        return Err(anyhow!(
                            "cache component world foreign package name must not be empty"
                        ));
                    }
                    if !package.interfaces_recorded {
                        return Err(anyhow!(
                            "cache component world foreign package interfaces are missing for '{}'",
                            package.name
                        ));
                    }
                    for interface_name in &package.interfaces {
                        if interface_name.trim().is_empty() {
                            return Err(anyhow!(
                                "cache component world foreign package interface name must not be empty for '{}'",
                                package.name
                            ));
                        }
                    }
                }
                let cache_sha = self.component_sha256.as_ref().ok_or_else(|| {
                    anyhow!("cache component sha256 is missing for wasm dependency")
                })?;
                plugin_sources::validate_sha256_hex(cache_sha, "cache component sha256")?;
                if self.component_source.is_none() {
                    return Err(anyhow!(
                        "cache component source is missing for wasm dependency"
                    ));
                }
                if let Some(component) = dependency.component.as_ref() {
                    if self.component_source.as_deref() != Some(component.source.as_str()) {
                        return Err(anyhow!(
                            "cache component source mismatch (cache='{}', config='{}')",
                            self.component_source.as_deref().unwrap_or(""),
                            component.source
                        ));
                    }
                    if self.component_registry != component.registry {
                        return Err(anyhow!(
                            "cache component registry mismatch (cache='{}', config='{}')",
                            self.component_registry.as_deref().unwrap_or(""),
                            component.registry.as_deref().unwrap_or("")
                        ));
                    }
                    if let Some(expected_sha) = component.sha256.as_deref()
                        && !cache_sha.eq_ignore_ascii_case(expected_sha)
                    {
                        return Err(anyhow!(
                            "cache component sha256 mismatch (cache='{}', config='{}')",
                            cache_sha,
                            expected_sha
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}

pub(crate) fn dependency_wit_target_rel(dependency_name: &str, version: &str) -> PathBuf {
    PathBuf::from("wit")
        .join("deps")
        .join(plugin_sources::wit_deps_dir_name(
            dependency_name,
            Some(version),
        ))
}

pub(crate) fn dependency_wit_path(dependency_name: &str, version: &str) -> String {
    plugin_sources::path_to_manifest_string(&dependency_wit_target_rel(dependency_name, version))
}

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

pub(crate) fn is_cache_hit(
    project_root: &Path,
    dependency: &ProjectDependency,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<bool> {
    let entry = match load_entry(project_root, &dependency.name) {
        Ok(entry) => entry,
        Err(_) => return Ok(false),
    };
    if dependency.wit.source_kind == plugin_sources::SourceKind::Path
        && entry.resolved_package_name.is_none()
    {
        return Ok(false);
    }
    if entry
        .validate_for_dependency(dependency, namespace_registries)
        .is_err()
    {
        return Ok(false);
    }
    if !entry_files_are_complete(project_root, &entry)? {
        return Ok(false);
    }

    if let Some(actual_fingerprint) = wit_source_fingerprint_if_exists(
        project_root,
        &dependency.wit.source,
        dependency.wit.source_kind,
    )? && entry.wit_source_fingerprint.as_deref() != Some(actual_fingerprint.as_str())
    {
        return Ok(false);
    }

    if let Some(component_source) = entry.component_source.as_deref()
        && let Some(actual_fingerprint) = component_source_fingerprint_if_exists(
            project_root,
            component_source,
            dependency
                .component
                .as_ref()
                .map(|component| component.source_kind)
                .unwrap_or(dependency.wit.source_kind),
        )?
        && entry.component_source_fingerprint.as_deref() != Some(actual_fingerprint.as_str())
    {
        return Ok(false);
    }

    Ok(true)
}

pub(crate) fn hydrate_project_wit_deps(
    project_root: &Path,
    dependencies: &[ProjectDependency],
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<()> {
    let mut hydrated_targets = Vec::with_capacity(dependencies.len());
    let mut entries = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let entry = load_entry(project_root, &dependency.name)?;
        entry
            .validate_for_dependency(dependency, namespace_registries)
            .map_err(|err| {
                anyhow!(
                    "dependency '{}' cache is stale under {}; {}: {err}",
                    dependency.name,
                    CACHE_ROOT_REL,
                    MISSING_CACHE_HINT
                )
            })?;

        if !entry_files_are_complete(project_root, &entry)? {
            return Err(anyhow!(
                "dependency '{}' cache is stale under {}; {}",
                dependency.name,
                CACHE_ROOT_REL,
                MISSING_CACHE_HINT
            ));
        }
        let hydrated_dependency_name = entry
            .resolved_package_name
            .as_deref()
            .unwrap_or(dependency.name.as_str());
        let hydrated_wit_path = dependency_wit_path(hydrated_dependency_name, &dependency.version);
        hydrated_targets.push((dependency.name.clone(), PathBuf::from(&hydrated_wit_path)));
        entries.push((dependency.name.clone(), entry, hydrated_wit_path));
    }
    validate_hydrated_wit_output_path_collisions(&hydrated_targets)?;

    let wit_root = project_root.join("wit").join("deps");
    if wit_root.exists() {
        fs::remove_dir_all(&wit_root)
            .with_context(|| format!("failed to reset wit root: {}", wit_root.display()))?;
    }
    fs::create_dir_all(&wit_root)
        .with_context(|| format!("failed to create wit root: {}", wit_root.display()))?;

    for (dependency_name, entry, hydrated_wit_path) in entries {
        let entry_root = cache_entry_root(project_root, &dependency_name);
        copy_tree_with_conflict_check(
            &entry_root.join(&entry.wit_path),
            &project_root.join(&hydrated_wit_path),
        )
        .with_context(|| {
            format!(
                "failed to hydrate direct wit package for dependency '{}'",
                dependency_name
            )
        })?;

        for transitive in &entry.transitive_packages {
            copy_tree_with_conflict_check(
                &entry_root.join(&transitive.path),
                &project_root.join(&transitive.path),
            )
            .with_context(|| {
                format!(
                    "failed to hydrate transitive wit package '{}' for dependency '{}'",
                    transitive.name, dependency_name
                )
            })?;
        }
    }

    Ok(())
}

pub(crate) fn verify_project_dependency_cache(
    project_root: &Path,
    dependencies: &[ProjectDependency],
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<()> {
    let mut hydrated_targets = Vec::with_capacity(dependencies.len());
    let mut entries = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let entry = load_entry(project_root, &dependency.name)?;
        entry
            .validate_for_dependency(dependency, namespace_registries)
            .map_err(|err| {
                anyhow!(
                    "dependency '{}' cache is stale under {}; {}: {err}",
                    dependency.name,
                    CACHE_ROOT_REL,
                    MISSING_CACHE_HINT
                )
            })?;
        let hydrated_dependency_name = entry
            .resolved_package_name
            .as_deref()
            .unwrap_or(dependency.name.as_str());
        let hydrated_wit_path = dependency_wit_path(hydrated_dependency_name, &dependency.version);
        hydrated_targets.push((dependency.name.clone(), PathBuf::from(&hydrated_wit_path)));
        entries.push((dependency.name.clone(), entry));
    }
    validate_hydrated_wit_output_path_collisions(&hydrated_targets)?;
    for (dependency_name, entry) in entries {
        if !entry_files_are_complete(project_root, &entry)? {
            return Err(anyhow!(
                "dependency '{}' cache is stale under {}; {}",
                dependency_name,
                CACHE_ROOT_REL,
                MISSING_CACHE_HINT
            ));
        }
    }
    Ok(())
}

fn validate_hydrated_wit_output_path_collisions(
    hydrated_targets: &[(String, PathBuf)],
) -> anyhow::Result<()> {
    let mut seen_targets: Vec<(String, PathBuf)> = Vec::with_capacity(hydrated_targets.len());
    for (dependency_name, target_rel) in hydrated_targets {
        for (existing_dependency, existing_target) in &seen_targets {
            if existing_target == target_rel {
                return Err(anyhow!(
                    "dependencies '{}' and '{}' both resolve to '{}'; dependency WIT output paths must be unique",
                    existing_dependency,
                    dependency_name,
                    plugin_sources::path_to_manifest_string(target_rel)
                ));
            }
            if target_rel.starts_with(existing_target) || existing_target.starts_with(target_rel) {
                return Err(anyhow!(
                    "dependencies '{}' and '{}' have overlapping WIT output paths ('{}' and '{}'); dependency WIT output paths must be disjoint",
                    existing_dependency,
                    dependency_name,
                    plugin_sources::path_to_manifest_string(existing_target),
                    plugin_sources::path_to_manifest_string(target_rel)
                ));
            }
        }
        seen_targets.push((dependency_name.clone(), target_rel.clone()));
    }
    Ok(())
}

pub(crate) fn resolve_cached_component_path(
    project_root: &Path,
    dependency_name: &str,
    expected_sha256: &str,
) -> anyhow::Result<PathBuf> {
    plugin_sources::validate_sha256_hex(expected_sha256, "component_sha256")?;
    let entry = load_entry(project_root, dependency_name)?;
    let cache_sha = entry.component_sha256.as_ref().ok_or_else(|| {
        anyhow!(
            "dependency '{}' component sha256 cache is missing under {}; {}",
            dependency_name,
            CACHE_ROOT_REL,
            MISSING_CACHE_HINT
        )
    })?;
    if !cache_sha.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            "dependency '{}' component sha256 cache mismatch (cache='{}', expected='{}'); {}",
            dependency_name,
            cache_sha,
            expected_sha256,
            MISSING_CACHE_HINT
        ));
    }

    let component_path = cache_component_path(project_root, dependency_name, cache_sha);
    if !component_path.is_file() {
        return Err(anyhow!(
            "dependency '{}' component cache is missing under {}; {}",
            dependency_name,
            CACHE_ROOT_REL,
            MISSING_CACHE_HINT
        ));
    }
    let actual_sha = compute_sha256_hex(&component_path)?;
    if !actual_sha.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            "dependency '{}' component cache hash mismatch (cache='{}', expected='{}'); {}",
            dependency_name,
            actual_sha,
            expected_sha256,
            MISSING_CACHE_HINT
        ));
    }
    Ok(component_path)
}

pub(crate) fn wit_source_fingerprint_if_exists(
    project_root: &Path,
    source: &str,
    source_kind: plugin_sources::SourceKind,
) -> anyhow::Result<Option<String>> {
    let Some(path) = resolve_existing_file_source_path(project_root, source, source_kind)? else {
        return Ok(None);
    };
    compute_path_digest_hex(&path).map(Some)
}

pub(crate) fn component_source_fingerprint_if_exists(
    project_root: &Path,
    source: &str,
    source_kind: plugin_sources::SourceKind,
) -> anyhow::Result<Option<String>> {
    let Some(path) = resolve_existing_file_source_path(project_root, source, source_kind)? else {
        return Ok(None);
    };
    let metadata = fs::metadata(&path)
        .with_context(|| format!("failed to inspect component source {}", path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!(
            "path component source must resolve to a file: {}",
            path.display()
        ));
    }
    compute_sha256_hex(&path).map(Some)
}

fn dependency_kind_label(kind: ManifestDependencyKind) -> &'static str {
    match kind {
        ManifestDependencyKind::Native => "native",
        ManifestDependencyKind::Wasm => "wasm",
    }
}

fn meta_path(project_root: &Path, dependency_name: &str) -> PathBuf {
    cache_entry_root(project_root, dependency_name).join("meta.toml")
}

fn validate_safe_wit_path(path: &str, field_name: &str) -> anyhow::Result<()> {
    let mut components = Path::new(path).components();
    let first = components.next();
    let second = components.next();
    if !matches!(first, Some(Component::Normal(v)) if v == "wit")
        || !matches!(second, Some(Component::Normal(v)) if v == "deps")
    {
        return Err(anyhow!(
            "{field_name} must start with 'wit/deps' (got '{}')",
            path
        ));
    }
    for component in components {
        if !matches!(component, Component::Normal(_)) {
            return Err(anyhow!(
                "{field_name} contains invalid path component (got '{}')",
                path
            ));
        }
    }
    Ok(())
}

fn parse_prefixed_sha256<'a>(value: &'a str, field_name: &str) -> anyhow::Result<&'a str> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(anyhow!("{field_name} must start with 'sha256:'"));
    };
    plugin_sources::validate_sha256_hex(hex, field_name)?;
    Ok(hex)
}

fn resolve_existing_file_source_path(
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
    let metadata = match fs::metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(anyhow!(
                "failed to inspect path source {}: {err}",
                resolved.display()
            ));
        }
    };
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(anyhow!(
            "resolved path source is not a file or directory: {}",
            resolved.display()
        ));
    }
    Ok(Some(resolved))
}

fn entry_files_are_complete(
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

fn copy_tree_with_conflict_check(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let metadata = fs::metadata(source)
        .with_context(|| format!("failed to inspect source path {}", source.display()))?;
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
        let file_name = source_path.file_name().ok_or_else(|| {
            anyhow!(
                "failed to resolve source file name under {}",
                source.display()
            )
        })?;
        let destination_path = destination.join(file_name);
        copy_tree_with_conflict_check(&source_path, &destination_path)?;
    }
    Ok(())
}

fn copy_file_with_conflict_check(source: &Path, destination: &Path) -> anyhow::Result<()> {
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

fn compute_sha256_hex(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_file_into(&mut hasher, path, "file for sha256")?;
    Ok(hex::encode(hasher.finalize()))
}

fn compute_path_digest_hex(path: &Path) -> anyhow::Result<String> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read path for digest: {}", path.display()))?;
    if metadata.is_file() {
        return compute_sha256_hex(path);
    }
    if !metadata.is_dir() {
        return Err(anyhow!("path is not file or directory: {}", path.display()));
    }

    let mut stack = vec![path.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory for digest: {}", dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read directory entry while hashing {}",
                    dir.display()
                )
            })?;
            let entry_path = entry.path();
            let entry_metadata = entry
                .metadata()
                .with_context(|| format!("failed to read metadata for {}", entry_path.display()))?;
            if entry_metadata.is_dir() {
                stack.push(entry_path);
            } else if entry_metadata.is_file() {
                files.push(entry_path);
            }
        }
    }
    files.sort();

    let mut hasher = Sha256::new();
    for file in files {
        let rel = file
            .strip_prefix(path)
            .with_context(|| format!("failed to relativize digest path: {}", file.display()))?;
        hasher.update(normalized_path_to_string(rel).as_bytes());
        hasher.update([0]);
        hash_file_into(&mut hasher, &file, "directory digest file")?;
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn normalized_path_to_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().replace('\\', "/")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn hash_file_into(hasher: &mut Sha256, path: &Path, context_label: &str) -> anyhow::Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(())
}
