use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use imago_lockfile::{ComponentExpectation, DependencyExpectation};

use crate::commands::{
    build::{self, ManifestDependencyKind},
    dependency_cache::{self, DependencyCacheEntry, DependencyCacheTransitivePackage},
    plugin_sources,
};

#[async_trait]
pub(crate) trait DependencyResolver {
    fn resolve_manifest_dependencies_from_lock(
        &self,
        project_root: &Path,
        dependencies: &[build::ProjectDependency],
    ) -> anyhow::Result<Vec<build::ManifestDependency>>;

    fn resolve_dependency_component_sources(
        &self,
        project_root: &Path,
        dependencies: &[build::ManifestDependency],
    ) -> anyhow::Result<BTreeMap<String, PathBuf>>;

    async fn load_or_refresh_cache_entry(
        &self,
        project_root: &Path,
        dependency: &build::ProjectDependency,
        namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
    ) -> anyhow::Result<DependencyCacheEntry>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StandardDependencyResolver;

#[async_trait]
impl DependencyResolver for StandardDependencyResolver {
    fn resolve_manifest_dependencies_from_lock(
        &self,
        project_root: &Path,
        dependencies: &[build::ProjectDependency],
    ) -> anyhow::Result<Vec<build::ManifestDependency>> {
        resolve_manifest_dependencies_from_lock(project_root, dependencies)
    }

    fn resolve_dependency_component_sources(
        &self,
        project_root: &Path,
        dependencies: &[build::ManifestDependency],
    ) -> anyhow::Result<BTreeMap<String, PathBuf>> {
        resolve_dependency_component_sources(project_root, dependencies)
    }

    async fn load_or_refresh_cache_entry(
        &self,
        project_root: &Path,
        dependency: &build::ProjectDependency,
        namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
    ) -> anyhow::Result<DependencyCacheEntry> {
        load_or_refresh_cache_entry(project_root, dependency, namespace_registries).await
    }
}

pub(crate) fn resolve_manifest_dependencies_from_lock(
    project_root: &Path,
    dependencies: &[build::ProjectDependency],
) -> anyhow::Result<Vec<build::ManifestDependency>> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let lock = imago_lockfile::load_from_project_root(project_root)?;
    let mut expectations = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        expectations.push(DependencyExpectation {
            name: dependency.name.clone(),
            version: dependency.version.clone(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: dependency.wit.registry.clone(),
            component: expected_component_for_dependency(dependency)?,
        });
    }
    let resolved_by_name =
        imago_lockfile::resolve_dependencies(project_root, &lock, &expectations)?;

    let mut manifest_dependencies = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let entry = resolved_by_name.get(&dependency.name).ok_or_else(|| {
            anyhow!(
                "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                dependency.name
            )
        })?;

        let component = match dependency.kind {
            ManifestDependencyKind::Native => None,
            ManifestDependencyKind::Wasm => {
                let lock_component_sha = entry.component_sha256.as_ref().ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' component sha256 is missing in imago.lock; run `imago deps sync`",
                        dependency.name
                    )
                })?;
                Some(build::ManifestDependencyComponent {
                    path: format!("plugins/components/{lock_component_sha}.wasm"),
                    sha256: lock_component_sha.clone(),
                })
            }
        };

        manifest_dependencies.push(build::ManifestDependency {
            name: dependency.name.clone(),
            version: dependency.version.clone(),
            kind: dependency.kind,
            wit: dependency.wit.source.clone(),
            requires: dependency.requires.clone(),
            component,
            capabilities: dependency.capabilities.clone(),
        });
    }

    Ok(manifest_dependencies)
}

pub(crate) fn expected_component_for_dependency(
    dependency: &build::ProjectDependency,
) -> anyhow::Result<Option<ComponentExpectation>> {
    match dependency.kind {
        ManifestDependencyKind::Native => Ok(None),
        ManifestDependencyKind::Wasm => {
            if let Some(component) = dependency.component.as_ref() {
                return Ok(Some(ComponentExpectation {
                    source: component.source.clone(),
                    registry: component.registry.clone(),
                    sha256: component.sha256.clone(),
                }));
            }
            let (source, registry) = plugin_sources::expected_component_identity_from_wit_source(
                &dependency.wit.source,
                dependency.wit.registry.as_deref(),
            )
            .with_context(|| {
                format!(
                    "dependency '{}' omits component settings but wit source '{}' cannot be mapped to a component source",
                    dependency.name, dependency.wit.source
                )
            })?;
            Ok(Some(ComponentExpectation {
                source,
                registry,
                sha256: None,
            }))
        }
    }
}

pub(crate) fn resolve_dependency_component_sources(
    project_root: &Path,
    dependencies: &[build::ManifestDependency],
) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    let mut sources = BTreeMap::new();
    if !dependencies
        .iter()
        .any(|dependency| dependency.kind == ManifestDependencyKind::Wasm)
    {
        return Ok(sources);
    }

    let lock = imago_lockfile::load_from_project_root(project_root)?;
    let lock_by_name = lock
        .dependencies
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect::<BTreeMap<_, _>>();

    for dependency in dependencies {
        if dependency.kind != ManifestDependencyKind::Wasm {
            continue;
        }

        let component = dependency.component.as_ref().ok_or_else(|| {
            anyhow!(
                "manifest dependency '{}' is missing component; run `imago deps sync`",
                dependency.name
            )
        })?;
        let lock_entry = lock_by_name.get(&dependency.name).ok_or_else(|| {
            anyhow!(
                "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                dependency.name
            )
        })?;
        lock_entry.component_source.as_deref().ok_or_else(|| {
            anyhow!(
                "dependency '{}' component source is missing in imago.lock; run `imago deps sync`",
                dependency.name
            )
        })?;
        let sha = lock_entry.component_sha256.as_deref().ok_or_else(|| {
            anyhow!(
                "dependency '{}' component sha256 is missing in imago.lock; run `imago deps sync`",
                dependency.name
            )
        })?;
        if component.sha256 != sha {
            return Err(anyhow!(
                "dependency '{}' component hash mismatch (manifest='{}', lock='{}'); run `imago deps sync`",
                dependency.name,
                component.sha256,
                sha
            ));
        }
        let cache_path =
            dependency_cache::resolve_cached_component_path(project_root, &dependency.name, sha)
                .with_context(|| {
                    format!(
                        "failed to resolve cached component bytes for dependency '{}'",
                        dependency.name
                    )
                })?;
        sources.insert(dependency.name.clone(), cache_path);
    }

    Ok(sources)
}

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

    let cache_wit_target =
        cache_entry_root.join(dependency_cache::dependency_wit_path(&dependency.name));
    fs::create_dir_all(&cache_wit_target).with_context(|| {
        format!(
            "failed to create dependency cache wit dir: {}",
            cache_wit_target.display()
        )
    })?;

    let materialized = plugin_sources::materialize_wit_source(
        project_root,
        &dependency.wit.source,
        dependency.wit.registry.as_deref(),
        namespace_registries,
        Some(dependency.name.as_str()),
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
    let wit_source_fingerprint =
        dependency_cache::wit_source_fingerprint_if_exists(project_root, &dependency.wit.source)
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
                        &component.source,
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
                    &source,
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
                let source_fingerprint =
                    dependency_cache::component_source_fingerprint_if_exists(project_root, &source)
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
        version: dependency.version.clone(),
        kind: match dependency.kind {
            ManifestDependencyKind::Native => "native".to_string(),
            ManifestDependencyKind::Wasm => "wasm".to_string(),
        },
        wit_source: dependency.wit.source.clone(),
        wit_registry: dependency.wit.registry.clone(),
        wit_path: dependency_cache::dependency_wit_path(&dependency.name),
        wit_digest: cache_wit_digest,
        wit_source_fingerprint,
        component_source,
        component_registry,
        component_sha256,
        component_source_fingerprint,
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
