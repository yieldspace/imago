use std::path::Path;

use std::fs;

use anyhow::{Context, anyhow};

use crate::commands::{
    build::{self, ManifestDependencyKind},
    dependency_cache::{
        self, DependencyCacheComponentWorldForeignPackage, DependencyCacheEntry,
        DependencyCacheTransitivePackage,
    },
    plugin_sources,
};

pub(crate) async fn load_or_refresh_cache_entry(
    project_root: &Path,
    dependency: &build::ProjectDependency,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<DependencyCacheEntry> {
    if dependency_cache::is_cache_hit(project_root, dependency, namespace_registries)? {
        return dependency_cache::load_entry(project_root, &dependency.name)
            .with_context(|| format!("failed to load dependency cache for '{}'", dependency.name));
    }

    let cache_entry_root = dependency_cache::cache_entry_root(project_root, &dependency.name);
    if cache_entry_root.exists() {
        fs::remove_dir_all(&cache_entry_root).with_context(|| {
            format!(
                "failed to reset dependency cache dir: {}",
                cache_entry_root.display()
            )
        })?;
    }

    let cache_wit_target = cache_entry_root.join(dependency_cache::dependency_wit_path(
        &dependency.name,
        &dependency.version,
    ));
    fs::create_dir_all(&cache_wit_target).with_context(|| {
        format!(
            "failed to create dependency cache wit dir: {}",
            cache_wit_target.display()
        )
    })?;

    let expected_package = if dependency.wit.source_kind == plugin_sources::SourceKind::Path {
        None
    } else {
        Some(dependency.name.as_str())
    };
    let materialized = plugin_sources::materialize_wit_source(
        project_root,
        dependency.wit.source_kind,
        &dependency.wit.source,
        Some(&dependency.version),
        dependency.wit.registry.as_deref(),
        namespace_registries,
        expected_package,
        dependency.wit.sha256.as_deref(),
        &cache_wit_target,
    )
    .await
    .with_context(|| format!("failed to resolve dependency '{}'", dependency.name))?;

    let cache_wit_digest =
        build::compute_path_digest_hex(&cache_wit_target).with_context(|| {
            format!(
                "failed to compute dependency cache wit digest: {}",
                cache_wit_target.display()
            )
        })?;
    let wit_source_fingerprint = dependency_cache::wit_source_fingerprint_if_exists(
        project_root,
        &dependency.wit.source,
        dependency.wit.source_kind,
    )
    .with_context(|| {
        format!(
            "failed to fingerprint wit source for dependency '{}'",
            dependency.name
        )
    })?;

    let (component_source, component_registry, component_sha256, component_source_fingerprint) =
        match dependency.kind {
            ManifestDependencyKind::Native => (None, None, None, None),
            ManifestDependencyKind::Wasm => {
                let (source, registry, sha256) = if let Some(component) =
                    dependency.component.as_ref()
                {
                    let digest = plugin_sources::resolve_component_sha256(
                        project_root,
                        component.source_kind,
                        &component.source,
                        Some(&dependency.version),
                        component.registry.as_deref(),
                        component.sha256.as_deref(),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to resolve component sha256 for dependency '{}'",
                            dependency.name
                        )
                    })?;
                    (component.source.clone(), component.registry.clone(), digest)
                } else if let Some(derived) = materialized.derived_component.as_ref() {
                    (
                        derived.source.clone(),
                        derived.registry.clone(),
                        derived.sha256.clone(),
                    )
                } else {
                    return Err(anyhow!(
                        "dependencies entry '{}' is kind=\"wasm\" but no component source was provided and wit source '{}' did not decode as a component",
                        dependency.name,
                        dependency.wit.source
                    ));
                };

                let cache_component_path =
                    dependency_cache::cache_component_path(project_root, &dependency.name, &sha256);
                plugin_sources::materialize_component_file(
                    project_root,
                    match dependency.component.as_ref() {
                        Some(component) => component.source_kind,
                        None => dependency.wit.source_kind,
                    },
                    &source,
                    Some(&dependency.version),
                    registry.as_deref(),
                    &sha256,
                    &cache_component_path,
                    "dependency component cache",
                )
                .await
                .with_context(|| {
                    format!(
                        "failed to materialize component cache for dependency '{}'",
                        dependency.name
                    )
                })?;
                let source_fingerprint = dependency_cache::component_source_fingerprint_if_exists(
                    project_root,
                    &source,
                    match dependency.component.as_ref() {
                        Some(component) => component.source_kind,
                        None => dependency.wit.source_kind,
                    },
                )
                .with_context(|| {
                    format!(
                        "failed to fingerprint component source for dependency '{}'",
                        dependency.name
                    )
                })?;
                (Some(source), registry, Some(sha256), source_fingerprint)
            }
        };

    let entry = DependencyCacheEntry {
        name: dependency.name.clone(),
        resolved_package_name: materialized.top_package_name.clone(),
        version: dependency.version.clone(),
        kind: match dependency.kind {
            ManifestDependencyKind::Native => "native".to_string(),
            ManifestDependencyKind::Wasm => "wasm".to_string(),
        },
        wit_source: dependency.wit.source.clone(),
        wit_registry: dependency.wit.registry.clone(),
        wit_sha256: dependency.wit.sha256.clone(),
        wit_path: dependency_cache::dependency_wit_path(&dependency.name, &dependency.version),
        wit_digest: cache_wit_digest,
        wit_source_fingerprint,
        component_source,
        component_registry,
        component_sha256,
        component_source_fingerprint,
        component_world_foreign_packages: materialized
            .component_world_foreign_packages
            .iter()
            .map(|package| DependencyCacheComponentWorldForeignPackage {
                name: package.name.clone(),
                version: package.version.clone(),
                interfaces: package.interfaces.clone(),
                interfaces_recorded: true,
            })
            .collect(),
        component_world_foreign_packages_recorded: true,
        transitive_packages: materialized
            .transitive_packages
            .iter()
            .map(|transitive| DependencyCacheTransitivePackage {
                name: transitive.name.clone(),
                registry: transitive.registry.clone(),
                requirement: transitive.requirement.clone(),
                version: transitive.version.clone(),
                digest: transitive.digest.clone(),
                source: transitive.source.clone(),
                path: transitive.path.clone(),
            })
            .collect(),
    };
    dependency_cache::save_entry(project_root, &entry)
        .with_context(|| format!("failed to save dependency cache for '{}'", dependency.name))?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::commands::{
        build::{
            ManifestCapabilityPolicy, ManifestDependencyKind, ProjectDependency,
            ProjectDependencySource, compute_path_digest_hex,
        },
        dependency_cache::{DependencyCacheEntry, cache_entry_root, save_entry},
    };

    use super::load_or_refresh_cache_entry;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-cache-refresh-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn sample_dependency(source: &str) -> ProjectDependency {
        ProjectDependency {
            name: "path-source-0".to_string(),
            version: "0.1.0".to_string(),
            kind: ManifestDependencyKind::Native,
            wit: ProjectDependencySource {
                source_kind: crate::commands::plugin_sources::SourceKind::Path,
                source: source.to_string(),
                registry: None,
                sha256: None,
            },
            requires: vec![],
            component: None,
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    fn write_cache_hit_fixture(
        root: &std::path::Path,
        dependency: &ProjectDependency,
    ) -> DependencyCacheEntry {
        let wit_path = crate::commands::dependency_cache::dependency_wit_path(
            &dependency.name,
            &dependency.version,
        );
        let entry_root = cache_entry_root(root, &dependency.name);
        let direct_wit = entry_root.join(&wit_path);
        fs::create_dir_all(&direct_wit).expect("wit path should be created");
        fs::write(
            direct_wit.join("package.wit"),
            b"package acme:example@0.1.0;\n",
        )
        .expect("package.wit should be created");
        let wit_digest = compute_path_digest_hex(&direct_wit).expect("digest should compute");

        let entry = DependencyCacheEntry {
            name: dependency.name.clone(),
            resolved_package_name: Some("acme:example".to_string()),
            version: dependency.version.clone(),
            kind: "native".to_string(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: None,
            wit_sha256: None,
            wit_path,
            wit_digest,
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        };
        save_entry(root, &entry).expect("cache entry should save");
        entry
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_or_refresh_cache_entry_returns_existing_entry_on_cache_hit() {
        let root = new_temp_dir("hit");
        let dependency = sample_dependency("registry/example");
        let expected = write_cache_hit_fixture(&root, &dependency);

        let entry = load_or_refresh_cache_entry(&root, &dependency, None)
            .await
            .expect("cache hit should load existing entry");
        assert_eq!(entry.name, expected.name);
        assert_eq!(entry.wit_digest, expected.wit_digest);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_or_refresh_cache_entry_attempts_refresh_on_cache_miss() {
        let root = new_temp_dir("miss");
        let dependency = sample_dependency("registry/missing");

        let err = load_or_refresh_cache_entry(&root, &dependency, None)
            .await
            .expect_err("cache miss with missing source should fail during refresh");
        assert!(err.to_string().contains("failed to resolve dependency"));
    }
}
