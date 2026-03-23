use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};

use crate::commands::{build::ProjectDependency, plugin_sources};

use super::{
    CACHE_ROOT_REL, DependencyCacheEntry, MISSING_CACHE_HINT,
    digest::{compute_path_digest_hex, compute_sha256_hex},
    io::{
        cache_component_path, cache_entry_root, copy_tree_with_conflict_check,
        entry_files_are_complete, load_entry, resolve_existing_file_source_path,
    },
};

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

pub(crate) fn is_cache_hit(
    project_root: &Path,
    dependency: &ProjectDependency,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<bool> {
    let entry = match load_entry(project_root, &dependency.name) {
        Ok(entry) => entry,
        Err(_) => return Ok(false),
    };
    if entry.resolved_package_name.is_none() {
        // Legacy cache entries predate resolved package tracking; refresh them so
        // dependency validation compares against the actual WIT package name.
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

    if ensure_cache_component_metadata_matches(project_root, dependency, &entry).is_err() {
        return Ok(false);
    }

    if ensure_cache_source_fingerprints_match(project_root, dependency, &entry).is_err() {
        return Ok(false);
    }

    Ok(true)
}

fn ensure_cache_component_metadata_matches(
    project_root: &Path,
    dependency: &ProjectDependency,
    entry: &DependencyCacheEntry,
) -> anyhow::Result<()> {
    if entry.kind != "wasm" {
        return Ok(());
    }
    let component_sha = entry
        .component_sha256
        .as_deref()
        .ok_or_else(|| anyhow!("cache component sha256 is missing for wasm dependency"))?;
    let component_path = cache_component_path(project_root, &entry.name, component_sha);
    let component_bytes = fs::read(&component_path).with_context(|| {
        format!(
            "failed to read cached component bytes for dependency '{}'",
            dependency.name
        )
    })?;
    let expected_package = match dependency.wit.source_kind {
        plugin_sources::SourceKind::Wit => Some(dependency.name.as_str()),
        plugin_sources::SourceKind::Oci | plugin_sources::SourceKind::Path => None,
    };
    let actual_resolved_name = plugin_sources::resolved_top_package_name_from_component_bytes(
        &component_bytes,
        expected_package,
        &format!("cached component '{}'", component_path.display()),
    )?;
    if let Some(actual_resolved_name) = actual_resolved_name
        && entry.resolved_package_name.as_deref() != Some(actual_resolved_name.as_str())
    {
        return Err(anyhow!(
            "cache resolved package name mismatch (cache='{}', actual='{}')",
            entry.resolved_package_name.as_deref().unwrap_or(""),
            actual_resolved_name
        ));
    }
    Ok(())
}

fn ensure_cache_source_fingerprints_match(
    project_root: &Path,
    dependency: &ProjectDependency,
    entry: &DependencyCacheEntry,
) -> anyhow::Result<()> {
    if let Some(actual_fingerprint) = wit_source_fingerprint_if_exists(
        project_root,
        &dependency.wit.source,
        dependency.wit.source_kind,
    )? && entry.wit_source_fingerprint.as_deref() != Some(actual_fingerprint.as_str())
    {
        return Err(anyhow!(
            "cache wit source fingerprint mismatch (cache='{}', actual='{}')",
            entry.wit_source_fingerprint.as_deref().unwrap_or(""),
            actual_fingerprint
        ));
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
        return Err(anyhow!(
            "cache component source fingerprint mismatch (cache='{}', actual='{}')",
            entry.component_source_fingerprint.as_deref().unwrap_or(""),
            actual_fingerprint
        ));
    }

    Ok(())
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
        ensure_cache_source_fingerprints_match(project_root, dependency, &entry).map_err(
            |err| {
                anyhow!(
                    "dependency '{}' cache is stale under {}; {}: {err}",
                    dependency.name,
                    CACHE_ROOT_REL,
                    MISSING_CACHE_HINT
                )
            },
        )?;

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
        ensure_cache_source_fingerprints_match(project_root, dependency, &entry).map_err(
            |err| {
                anyhow!(
                    "dependency '{}' cache is stale under {}; {}: {err}",
                    dependency.name,
                    CACHE_ROOT_REL,
                    MISSING_CACHE_HINT
                )
            },
        )?;
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

pub(super) fn validate_hydrated_wit_output_path_collisions(
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

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use sha2::Digest as _;

    use crate::commands::dependency_cache::{
        DependencyCacheEntry, DependencyCacheTransitivePackage, cache_component_path, save_entry,
    };
    use crate::commands::{
        build::{
            ManifestCapabilityPolicy, ManifestDependencyKind, ProjectDependency,
            ProjectDependencyComponent, ProjectDependencySource,
        },
        plugin_sources,
    };

    use super::{
        component_source_fingerprint_if_exists, is_cache_hit, resolve_cached_component_path,
        validate_hydrated_wit_output_path_collisions, verify_project_dependency_cache,
        wit_source_fingerprint_if_exists,
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-dependency-cache-hydrate-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &std::path::Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn encode_wit_component(root: &std::path::Path, world: &str) -> Vec<u8> {
        let mut resolve = wit_parser::Resolve::default();
        let (pkg, _) = resolve
            .push_dir(root)
            .expect("fixture WIT directory should parse");
        let world_id = resolve
            .select_world(&[pkg], Some(world))
            .expect("fixture world should exist");
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        wit_component::embed_component_metadata(
            &mut module,
            &resolve,
            world_id,
            wit_component::StringEncoding::UTF8,
        )
        .expect("component metadata embedding should succeed");
        wit_component::ComponentEncoder::default()
            .module(&module)
            .expect("component encoder should accept module")
            .encode()
            .expect("component encoding should succeed")
    }

    fn sample_entry(component_sha256: &str) -> DependencyCacheEntry {
        DependencyCacheEntry {
            name: "path-source-0".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "registry/example".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: "wit/deps/path-source-0-0.1.0".to_string(),
            wit_digest: "digest".to_string(),
            wit_source_fingerprint: None,
            component_source: Some("registry/example-component.wasm".to_string()),
            component_registry: None,
            component_sha256: Some(component_sha256.to_string()),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![DependencyCacheTransitivePackage {
                name: "wasi:io".to_string(),
                registry: None,
                requirement: "^0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: format!("sha256:{}", "a".repeat(64)),
                source: None,
                path: "wit/deps/wasi-io-0.2.0".to_string(),
            }],
        }
    }

    fn sample_dependency() -> ProjectDependency {
        ProjectDependency {
            name: "path-source-0".to_string(),
            version: "0.1.0".to_string(),
            kind: ManifestDependencyKind::Wasm,
            wit: ProjectDependencySource {
                source_kind: plugin_sources::SourceKind::Path,
                source: "registry/example".to_string(),
                registry: None,
                sha256: None,
            },
            requires: vec![],
            component: Some(ProjectDependencyComponent {
                source_kind: plugin_sources::SourceKind::Path,
                source: "registry/example-component.wasm".to_string(),
                registry: None,
                sha256: None,
            }),
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    fn sample_wit_dependency() -> ProjectDependency {
        ProjectDependency {
            name: "acme:camera".to_string(),
            version: "0.1.0".to_string(),
            kind: ManifestDependencyKind::Wasm,
            wit: ProjectDependencySource {
                source_kind: plugin_sources::SourceKind::Wit,
                source: "acme:camera".to_string(),
                registry: Some("wa.dev".to_string()),
                sha256: None,
            },
            requires: vec![],
            component: Some(ProjectDependencyComponent {
                source_kind: plugin_sources::SourceKind::Wit,
                source: "acme:camera".to_string(),
                registry: Some("wa.dev".to_string()),
                sha256: None,
            }),
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    #[test]
    fn validate_hydrated_wit_output_path_collisions_rejects_duplicate_and_overlap() {
        let duplicate = vec![
            (
                "dep-a".to_string(),
                PathBuf::from("wit/deps/path-source-0-0.1.0"),
            ),
            (
                "dep-b".to_string(),
                PathBuf::from("wit/deps/path-source-0-0.1.0"),
            ),
        ];
        let duplicate_err = validate_hydrated_wit_output_path_collisions(&duplicate)
            .expect_err("duplicate output path should fail");
        assert!(duplicate_err.to_string().contains("both resolve"));

        let overlap = vec![
            ("dep-a".to_string(), PathBuf::from("wit/deps/a")),
            ("dep-b".to_string(), PathBuf::from("wit/deps/a/nested")),
        ];
        let overlap_err = validate_hydrated_wit_output_path_collisions(&overlap)
            .expect_err("overlapping output paths should fail");
        assert!(
            overlap_err
                .to_string()
                .contains("overlapping WIT output paths")
        );
    }

    #[test]
    fn resolve_cached_component_path_validates_sha_and_returns_component_file() {
        let root = new_temp_dir("resolve-component");
        let component_bytes = b"\0asm\x01\0\0\0";
        let provisional = cache_component_path(&root, "path-source-0", "provisional");
        if let Some(parent) = provisional.parent() {
            fs::create_dir_all(parent).expect("component cache dir should be created");
        }
        fs::write(&provisional, component_bytes).expect("component should be written");
        let component_sha = super::compute_sha256_hex(&provisional).expect("sha should compute");
        fs::remove_file(&provisional).expect("provisional file should be removed");

        let entry = sample_entry(&component_sha);
        save_entry(&root, &entry).expect("entry should be saved");

        let component_path = cache_component_path(&root, &entry.name, &component_sha);
        if let Some(parent) = component_path.parent() {
            fs::create_dir_all(parent).expect("component cache dir should be created");
        }
        fs::write(&component_path, component_bytes).expect("component should be written");

        let resolved = resolve_cached_component_path(&root, &entry.name, &component_sha)
            .expect("cache path should resolve");
        assert_eq!(resolved, component_path);

        let mismatch = resolve_cached_component_path(&root, &entry.name, &"b".repeat(64))
            .expect_err("sha mismatch should fail");
        assert!(mismatch.to_string().contains("cache mismatch"));
    }

    #[test]
    fn is_cache_hit_rejects_legacy_entry_without_resolved_package_name() {
        let root = new_temp_dir("legacy-entry-without-resolved-package-name");
        let dependency = sample_wit_dependency();
        let entry = DependencyCacheEntry {
            name: dependency.name.clone(),
            resolved_package_name: None,
            version: dependency.version.clone(),
            kind: "wasm".to_string(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: dependency.wit.registry.clone(),
            wit_sha256: dependency.wit.sha256.clone(),
            wit_path: "wit/deps/acme-camera-0.1.0".to_string(),
            wit_digest: "digest".to_string(),
            wit_source_fingerprint: None,
            component_source: dependency.component.as_ref().map(|v| v.source.clone()),
            component_registry: dependency
                .component
                .as_ref()
                .and_then(|v| v.registry.clone()),
            component_sha256: Some("a".repeat(64)),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        };
        save_entry(&root, &entry).expect("legacy cache entry should be saved");

        let hit = is_cache_hit(&root, &dependency, None).expect("cache hit check should succeed");
        assert!(!hit, "legacy cache entry must be refreshed");
    }

    #[test]
    fn is_cache_hit_rejects_outdated_root_component_resolution_for_component_dependency() {
        let root = new_temp_dir("outdated-root-component-resolution");
        let dependency = sample_dependency();
        let fixture = root.join("fixture");
        write(
            &fixture.join("package.wit"),
            br#"
    package root:component@0.1.0;

    world camera-plugin {
      import imago:usb/provider@0.3.0;
      export imago:camera/types@0.1.0;
      export imago:camera/provider@0.1.0;
    }
    "#,
        );
        write(
            &fixture.join("deps/imago-camera/package.wit"),
            br#"
    package imago:camera@0.1.0;

    interface types {}

    interface provider {}
    "#,
        );
        write(
            &fixture.join("deps/imago-usb/package.wit"),
            br#"
    package imago:usb@0.3.0;

    interface provider {}
    "#,
        );
        let component_bytes = encode_wit_component(&fixture, "camera-plugin");
        let component_sha = hex::encode(sha2::Sha256::digest(&component_bytes));
        let component_path = cache_component_path(&root, &dependency.name, &component_sha);
        if let Some(parent) = component_path.parent() {
            fs::create_dir_all(parent).expect("component cache dir should be created");
        }
        fs::write(&component_path, &component_bytes).expect("component should be written");

        let entry = DependencyCacheEntry {
            name: dependency.name.clone(),
            resolved_package_name: Some("root:component".to_string()),
            version: dependency.version.clone(),
            kind: "wasm".to_string(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: dependency.wit.registry.clone(),
            wit_sha256: dependency.wit.sha256.clone(),
            wit_path: "wit/deps/path-source-0-0.1.0".to_string(),
            wit_digest: "digest".to_string(),
            wit_source_fingerprint: None,
            component_source: dependency.component.as_ref().map(|v| v.source.clone()),
            component_registry: dependency
                .component
                .as_ref()
                .and_then(|v| v.registry.clone()),
            component_sha256: Some(component_sha),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        };
        save_entry(&root, &entry).expect("cache entry should be saved");

        let hit = is_cache_hit(&root, &dependency, None).expect("cache hit check should succeed");
        assert!(
            !hit,
            "outdated root:component cache entry must be refreshed"
        );
    }

    #[test]
    fn wit_source_fingerprint_if_exists_returns_none_when_source_path_is_missing() {
        let root = new_temp_dir("fingerprint-none");
        let fingerprint = wit_source_fingerprint_if_exists(
            &root,
            "registry/missing",
            crate::commands::plugin_sources::SourceKind::Path,
        )
        .expect("missing path should be treated as none");
        assert!(fingerprint.is_none());
    }

    #[test]
    fn verify_project_dependency_cache_rejects_component_source_fingerprint_drift() {
        let root = new_temp_dir("fingerprint-drift");
        fs::create_dir_all(root.join("registry/example")).expect("source dir should exist");
        fs::write(
            root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\n",
        )
        .expect("wit source should be written");
        fs::write(
            root.join("registry/example-component.wasm"),
            b"\0asm\x01\0\0\0",
        )
        .expect("component source should be written");

        let component_sha =
            super::compute_sha256_hex(&root.join("registry/example-component.wasm"))
                .expect("component sha should compute");
        let mut entry = sample_entry(&component_sha);
        entry.transitive_packages.clear();
        entry.wit_source_fingerprint = wit_source_fingerprint_if_exists(
            &root,
            "registry/example",
            plugin_sources::SourceKind::Path,
        )
        .expect("wit fingerprint should compute");
        entry.component_source_fingerprint = component_source_fingerprint_if_exists(
            &root,
            "registry/example-component.wasm",
            plugin_sources::SourceKind::Path,
        )
        .expect("component fingerprint should compute");

        let cache_wit_dir = root.join(".imago/deps/path-source-0").join(&entry.wit_path);
        fs::create_dir_all(&cache_wit_dir).expect("cache wit dir should exist");
        fs::write(
            cache_wit_dir.join("package.wit"),
            b"package test:example@0.1.0;\n",
        )
        .expect("cache wit should be written");
        entry.wit_digest = super::compute_path_digest_hex(&cache_wit_dir)
            .expect("cache wit digest should compute");

        let component_path = cache_component_path(&root, &entry.name, &component_sha);
        if let Some(parent) = component_path.parent() {
            fs::create_dir_all(parent).expect("component cache dir should be created");
        }
        fs::write(&component_path, b"\0asm\x01\0\0\0").expect("component cache should exist");
        save_entry(&root, &entry).expect("entry should be saved");

        verify_project_dependency_cache(&root, &[sample_dependency()], None)
            .expect("matching fingerprints should pass");

        fs::write(
            root.join("registry/example-component.wasm"),
            b"\0asm\x01\0\0\0drift",
        )
        .expect("component source should drift");
        let err = verify_project_dependency_cache(&root, &[sample_dependency()], None)
            .expect_err("drifted component source should fail cache verification");
        assert!(
            err.to_string()
                .contains("cache component source fingerprint mismatch")
        );
    }
}
