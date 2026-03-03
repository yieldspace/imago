use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use async_trait::async_trait;

use crate::commands::{build, dependency_cache::DependencyCacheEntry, plugin_sources};

mod cache_refresh;
mod expectation;
mod lock_resolution;

pub(crate) use cache_refresh::load_or_refresh_cache_entry;
pub(crate) use expectation::dependency_expectation_for_project_dependency;
pub(crate) use lock_resolution::{
    resolve_dependency_component_sources, resolve_manifest_dependencies_from_lock,
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

#[cfg(test)]
mod tests {
    use crate::commands::{
        build::{
            ManifestCapabilityPolicy, ManifestDependencyKind, ProjectDependency,
            ProjectDependencyComponent, ProjectDependencySource,
        },
        plugin_sources,
    };

    use super::dependency_expectation_for_project_dependency;

    fn sample_native_dependency() -> ProjectDependency {
        ProjectDependency {
            name: "example:native".to_string(),
            version: "1.2.3".to_string(),
            kind: ManifestDependencyKind::Native,
            wit: ProjectDependencySource {
                source_kind: plugin_sources::SourceKind::Wit,
                source: "example:pkg".to_string(),
                registry: Some("wa.dev".to_string()),
                sha256: None,
            },
            requires: vec!["wasi:io".to_string()],
            component: None,
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    #[test]
    fn dependency_expectation_maps_project_dependency_fields() {
        let dependency = sample_native_dependency();
        let expectation = dependency_expectation_for_project_dependency(&dependency)
            .expect("expectation generation should succeed");

        assert_eq!(expectation.name, dependency.name);
        assert_eq!(expectation.version, dependency.version);
        assert_eq!(expectation.source, dependency.wit.source);
        assert_eq!(expectation.registry, dependency.wit.registry);
        assert!(expectation.component.is_none());
    }

    #[test]
    fn dependency_expectation_derives_component_for_wasm_when_component_is_omitted() {
        let mut dependency = sample_native_dependency();
        dependency.kind = ManifestDependencyKind::Wasm;
        dependency.wit.source_kind = plugin_sources::SourceKind::Path;
        dependency.wit.source = "registry/example".to_string();
        dependency.wit.registry = None;
        dependency.component = None;

        let expectation = dependency_expectation_for_project_dependency(&dependency)
            .expect("wasm dependency should derive component expectation");
        let component = expectation
            .component
            .expect("component expectation must be present for wasm dependency");

        assert_eq!(component.source, "registry/example");
        assert!(component.registry.is_none());
    }

    #[test]
    fn dependency_expectation_prefers_explicit_component_when_present() {
        let mut dependency = sample_native_dependency();
        dependency.kind = ManifestDependencyKind::Wasm;
        dependency.wit.source_kind = plugin_sources::SourceKind::Path;
        dependency.wit.source = "registry/example".to_string();
        dependency.wit.registry = None;
        dependency.component = Some(ProjectDependencyComponent {
            source_kind: plugin_sources::SourceKind::Path,
            source: "registry/explicit-component.wasm".to_string(),
            registry: None,
            sha256: Some("c".repeat(64)),
        });

        let expectation = dependency_expectation_for_project_dependency(&dependency)
            .expect("expectation generation should succeed");
        let component = expectation
            .component
            .expect("component expectation should be present");
        let expected_sha = "c".repeat(64);

        assert_eq!(component.source, "registry/explicit-component.wasm");
        assert_eq!(component.sha256.as_deref(), Some(expected_sha.as_str()));
    }
}
