use crate::lockfile::{
    ComponentExpectation, DependencyExpectation, LockCapabilityPolicy, LockDependencyKind,
    LockSourceKind,
};
use anyhow::Context;

use crate::commands::{
    build::{self, ManifestDependencyKind},
    plugin_sources,
};

pub(crate) fn expected_component_for_dependency(
    dependency: &build::ProjectDependency,
) -> anyhow::Result<Option<ComponentExpectation>> {
    match dependency.kind {
        ManifestDependencyKind::Native => Ok(None),
        ManifestDependencyKind::Wasm => {
            if let Some(component) = dependency.component.as_ref() {
                return Ok(Some(ComponentExpectation {
                    source_kind: lock_source_kind(component.source_kind),
                    source: component.source.clone(),
                    registry: component.registry.clone(),
                    sha256: component.sha256.clone(),
                }));
            }
            let (source, registry) = plugin_sources::expected_component_identity_from_wit_source(
                dependency.wit.source_kind,
                &dependency.wit.source,
                Some(&dependency.version),
                dependency.wit.registry.as_deref(),
            )
            .with_context(|| {
                format!(
                    "dependency '{}' omits component settings but wit source '{}' cannot be mapped to a component source",
                    dependency.name, dependency.wit.source
                )
            })?;
            Ok(Some(ComponentExpectation {
                source_kind: lock_source_kind(dependency.wit.source_kind),
                source,
                registry,
                sha256: None,
            }))
        }
    }
}

pub(crate) fn dependency_expectation_for_project_dependency(
    dependency: &build::ProjectDependency,
) -> anyhow::Result<DependencyExpectation> {
    Ok(DependencyExpectation {
        name: dependency.name.clone(),
        kind: lock_dependency_kind(dependency.kind),
        version: dependency.version.clone(),
        source_kind: lock_source_kind(dependency.wit.source_kind),
        source: dependency.wit.source.clone(),
        registry: dependency.wit.registry.clone(),
        sha256: dependency.wit.sha256.clone(),
        requires: dependency.requires.clone(),
        capabilities: lock_capability_policy(&dependency.capabilities),
        component: expected_component_for_dependency(dependency)?,
    })
}

fn lock_source_kind(kind: plugin_sources::SourceKind) -> LockSourceKind {
    match kind {
        plugin_sources::SourceKind::Wit => LockSourceKind::Wit,
        plugin_sources::SourceKind::Oci => LockSourceKind::Oci,
        plugin_sources::SourceKind::Path => LockSourceKind::Path,
    }
}

fn lock_dependency_kind(kind: ManifestDependencyKind) -> LockDependencyKind {
    match kind {
        ManifestDependencyKind::Native => LockDependencyKind::Native,
        ManifestDependencyKind::Wasm => LockDependencyKind::Wasm,
    }
}

fn lock_capability_policy(policy: &build::ManifestCapabilityPolicy) -> LockCapabilityPolicy {
    LockCapabilityPolicy {
        privileged: policy.privileged,
        deps: policy.deps.clone(),
        wasi: policy.wasi.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        commands::{
            build::{
                ManifestCapabilityPolicy, ManifestDependencyKind, ProjectDependency,
                ProjectDependencyComponent, ProjectDependencySource,
            },
            plugin_sources,
        },
        lockfile::{LockDependencyKind, LockSourceKind},
    };

    use super::{dependency_expectation_for_project_dependency, expected_component_for_dependency};

    fn sample_dependency(kind: ManifestDependencyKind) -> ProjectDependency {
        ProjectDependency {
            name: "path-source-0".to_string(),
            version: "0.1.0".to_string(),
            kind,
            wit: ProjectDependencySource {
                source_kind: plugin_sources::SourceKind::Path,
                source: "registry/example".to_string(),
                registry: None,
                sha256: None,
            },
            requires: vec!["wasi:io".to_string()],
            component: None,
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    #[test]
    fn expected_component_for_dependency_prefers_explicit_component() {
        let mut dependency = sample_dependency(ManifestDependencyKind::Wasm);
        let explicit_sha = "a".repeat(64);
        dependency.component = Some(ProjectDependencyComponent {
            source_kind: plugin_sources::SourceKind::Path,
            source: "registry/explicit.wasm".to_string(),
            registry: None,
            sha256: Some(explicit_sha.clone()),
        });

        let component = expected_component_for_dependency(&dependency)
            .expect("component expectation should build")
            .expect("component should exist");
        assert_eq!(component.source_kind, LockSourceKind::Path);
        assert_eq!(component.source, "registry/explicit.wasm");
        assert_eq!(component.sha256.as_deref(), Some(explicit_sha.as_str()));
    }

    #[test]
    fn expected_component_for_dependency_derives_component_from_wit_source_when_omitted() {
        let dependency = sample_dependency(ManifestDependencyKind::Wasm);
        let component = expected_component_for_dependency(&dependency)
            .expect("derived component should build")
            .expect("component should exist");
        assert_eq!(component.source_kind, LockSourceKind::Path);
        assert_eq!(component.source, "registry/example");
        assert!(component.sha256.is_none());
    }

    #[test]
    fn dependency_expectation_for_project_dependency_maps_capabilities_and_kind() {
        let mut dependency = sample_dependency(ManifestDependencyKind::Native);
        dependency.capabilities.privileged = true;
        dependency
            .capabilities
            .wasi
            .insert("http".to_string(), vec!["outbound".to_string()]);

        let expectation = dependency_expectation_for_project_dependency(&dependency)
            .expect("expectation should build");
        assert_eq!(expectation.name, dependency.name);
        assert_eq!(expectation.kind, LockDependencyKind::Native);
        assert!(expectation.capabilities.privileged);
        assert_eq!(
            expectation.capabilities.wasi.get("http"),
            Some(&vec!["outbound".to_string()])
        );
    }
}
