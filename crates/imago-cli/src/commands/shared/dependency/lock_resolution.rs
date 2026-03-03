use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};

use crate::commands::{
    build::{self, ManifestDependencyKind},
    dependency_cache,
};

use super::dependency_expectation_for_project_dependency;

pub(crate) fn resolve_manifest_dependencies_from_lock(
    project_root: &Path,
    dependencies: &[build::ProjectDependency],
) -> anyhow::Result<Vec<build::ManifestDependency>> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let lock = crate::lockfile::load_from_project_root(project_root)?;
    let expectations = dependencies
        .iter()
        .map(dependency_expectation_for_project_dependency)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let resolved_by_name =
        crate::lockfile::resolve_dependencies(project_root, &lock, &expectations)?;
    let mut resolved_name_by_request_id = BTreeMap::new();
    let mut project_dependency_id_by_resolved_name = BTreeMap::new();
    for (project_dependency_id, entry) in &resolved_by_name {
        resolved_name_by_request_id.insert(entry.request_id.clone(), entry.resolved_name.clone());
        if let Some(existing_project_dependency_id) = project_dependency_id_by_resolved_name
            .insert(entry.resolved_name.clone(), project_dependency_id.clone())
        {
            return Err(anyhow!(
                "imago.lock resolves multiple project dependency ids ('{}', '{}') to package '{}'; run `imago deps sync`",
                existing_project_dependency_id,
                project_dependency_id,
                entry.resolved_name
            ));
        }
    }

    let mut manifest_dependencies = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let entry = resolved_by_name.get(&dependency.name).ok_or_else(|| {
            anyhow!(
                "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                dependency.name
            )
        })?;
        let requires = entry
            .requires_request_ids
            .iter()
            .map(|request_id| {
                resolved_name_by_request_id.get(request_id).cloned().ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' requires unresolved request_id '{}' in imago.lock; run `imago deps sync`",
                        dependency.name,
                        request_id
                    )
                })
            })
            .collect::<anyhow::Result<BTreeSet<_>>>()?
            .into_iter()
            .collect::<Vec<_>>();

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
            name: entry.resolved_name.clone(),
            version: dependency.version.clone(),
            kind: dependency.kind,
            wit: dependency.wit.source.clone(),
            requires,
            component,
            capabilities: dependency.capabilities.clone(),
        });
    }

    Ok(manifest_dependencies)
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

    let project_dependencies = build::load_project_dependencies_with_namespace_registries(
        project_root,
        &build::load_namespace_registries(project_root)?,
    )?;
    let lock = crate::lockfile::load_from_project_root(project_root)?;
    let expectations = project_dependencies
        .iter()
        .map(dependency_expectation_for_project_dependency)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let resolved_by_name =
        crate::lockfile::resolve_dependencies(project_root, &lock, &expectations)?;
    let mut project_dependency_id_by_resolved_name = BTreeMap::new();
    for (project_dependency_id, resolved) in &resolved_by_name {
        if let Some(existing_project_dependency_id) = project_dependency_id_by_resolved_name.insert(
            resolved.resolved_name.clone(),
            project_dependency_id.clone(),
        ) {
            return Err(anyhow!(
                "imago.lock resolves multiple project dependency ids ('{}', '{}') to package '{}'; run `imago deps sync`",
                existing_project_dependency_id,
                project_dependency_id,
                resolved.resolved_name
            ));
        }
    }

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
        let project_dependency_id = project_dependency_id_by_resolved_name
            .get(&dependency.name)
            .ok_or_else(|| {
                anyhow!(
                    "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                    dependency.name
                )
            })?;
        let lock_entry = resolved_by_name.get(project_dependency_id).ok_or_else(|| {
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
        let cache_path = dependency_cache::resolve_cached_component_path(
            project_root,
            project_dependency_id,
            sha,
        )
        .with_context(|| {
            format!(
                "failed to resolve cached component bytes for dependency '{}' (project dependency id '{}')",
                dependency.name, project_dependency_id
            )
        })?;
        sources.insert(dependency.name.clone(), cache_path);
    }

    Ok(sources)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use crate::{
        commands::build::{
            ManifestCapabilityPolicy, ManifestDependency, ManifestDependencyComponent,
            ManifestDependencyKind, ProjectDependency, ProjectDependencyComponent,
            ProjectDependencySource,
        },
        lockfile::{
            DependencyExpectation, IMAGO_LOCK_VERSION, ImagoLock, ImagoLockResolved,
            ImagoLockResolvedDependency, LockCapabilityPolicy, LockDependencyKind, LockSourceKind,
            build_requested_snapshot, compute_dependency_request_id,
        },
    };

    use super::{
        super::dependency_expectation_for_project_dependency, resolve_dependency_component_sources,
        resolve_manifest_dependencies_from_lock,
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-lock-resolution-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn sample_project_dependency(kind: ManifestDependencyKind) -> ProjectDependency {
        ProjectDependency {
            name: "path-source-0".to_string(),
            version: "0.1.0".to_string(),
            kind,
            wit: ProjectDependencySource {
                source_kind: crate::commands::plugin_sources::SourceKind::Path,
                source: "registry/example".to_string(),
                registry: None,
                sha256: None,
            },
            requires: vec![],
            component: match kind {
                ManifestDependencyKind::Native => None,
                ManifestDependencyKind::Wasm => Some(ProjectDependencyComponent {
                    source_kind: crate::commands::plugin_sources::SourceKind::Path,
                    source: "registry/example-component.wasm".to_string(),
                    registry: None,
                    sha256: None,
                }),
            },
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    fn write_lock(path: &Path, lock: &ImagoLock) {
        let encoded = toml::to_string_pretty(lock).expect("lock should serialize");
        write(&path.join("imago.lock"), encoded.as_bytes());
    }

    #[test]
    fn resolve_manifest_dependencies_from_lock_rejects_missing_resolved_entry() {
        let root = new_temp_dir("missing-resolved");
        let dependency = sample_project_dependency(ManifestDependencyKind::Native);
        let expectation = dependency_expectation_for_project_dependency(&dependency)
            .expect("expectation should build");
        let requested = build_requested_snapshot(std::slice::from_ref(&expectation), &[], None)
            .expect("requested snapshot should build");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };
        write_lock(&root, &lock);

        let err = resolve_manifest_dependencies_from_lock(&root, &[dependency])
            .expect_err("missing resolved dependency should fail");
        assert!(err.to_string().contains("is not resolved in imago.lock"));
    }

    #[test]
    fn resolve_dependency_component_sources_rejects_manifest_lock_component_sha_mismatch() {
        let root = new_temp_dir("component-sha-mismatch");
        write(
            &root.join("imago.toml"),
            br#"
[[dependencies]]
version = "0.1.0"
kind = "wasm"
path = "registry/example"

[dependencies.component]
path = "registry/example-component.wasm"
"#,
        );

        let project_dependency = sample_project_dependency(ManifestDependencyKind::Wasm);
        let expectation: DependencyExpectation =
            dependency_expectation_for_project_dependency(&project_dependency)
                .expect("expectation should build");
        let request_id = compute_dependency_request_id(&expectation);
        let requested = build_requested_snapshot(std::slice::from_ref(&expectation), &[], None)
            .expect("requested snapshot should build");

        let wit_path = "wit/deps/path-source-0-0.1.0";
        write(
            &root.join(wit_path).join("package.wit"),
            b"package acme:example@0.1.0;\n",
        );
        let wit_tree_digest = crate::commands::build::compute_path_digest_hex(&root.join(wit_path))
            .expect("wit digest should compute");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![ImagoLockResolvedDependency {
                    request_id,
                    resolved_name: "acme:example".to_string(),
                    resolved_version: "0.1.0".to_string(),
                    wit_path: wit_path.to_string(),
                    wit_tree_digest,
                    component_source: Some("registry/example-component.wasm".to_string()),
                    component_registry: None,
                    component_sha256: Some("a".repeat(64)),
                    requires_request_ids: vec![],
                }],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };
        write_lock(&root, &lock);

        let manifest_dependencies = vec![ManifestDependency {
            name: "acme:example".to_string(),
            version: "0.1.0".to_string(),
            kind: ManifestDependencyKind::Wasm,
            wit: "registry/example".to_string(),
            requires: vec![],
            component: Some(ManifestDependencyComponent {
                path: "plugins/components/bad.wasm".to_string(),
                sha256: "b".repeat(64),
            }),
            capabilities: ManifestCapabilityPolicy::default(),
        }];

        let err = resolve_dependency_component_sources(&root, &manifest_dependencies)
            .expect_err("manifest/lock sha mismatch should fail");
        assert!(err.to_string().contains("component hash mismatch"));
    }

    #[test]
    fn lock_resolution_reexports_dependency_expectation_builder() {
        let dependency = ProjectDependency {
            name: "dep".to_string(),
            version: "1.0.0".to_string(),
            kind: ManifestDependencyKind::Native,
            wit: ProjectDependencySource {
                source_kind: crate::commands::plugin_sources::SourceKind::Wit,
                source: "acme:dep".to_string(),
                registry: Some("wa.dev".to_string()),
                sha256: None,
            },
            requires: vec![],
            component: None,
            capabilities: ManifestCapabilityPolicy {
                privileged: false,
                deps: BTreeMap::new(),
                wasi: BTreeMap::new(),
            },
        };

        let expectation = dependency_expectation_for_project_dependency(&dependency)
            .expect("expectation should build");
        assert_eq!(expectation.kind, LockDependencyKind::Native);
        assert_eq!(expectation.source_kind, LockSourceKind::Wit);
        assert_eq!(expectation.capabilities, LockCapabilityPolicy::default());
    }
}
