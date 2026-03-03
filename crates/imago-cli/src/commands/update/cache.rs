use std::path::Path;

use crate::commands::{
    build, dependency_cache::DependencyCacheEntry, shared::dependency::DependencyResolver,
};

pub(crate) async fn load_or_refresh_cache_entry<R: DependencyResolver>(
    resolver: &R,
    project_root: &Path,
    dependency: &build::ProjectDependency,
    namespace_registries: Option<&crate::commands::plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<DependencyCacheEntry> {
    resolver
        .load_or_refresh_cache_entry(project_root, dependency, namespace_registries)
        .await
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;

    use crate::commands::{
        build::{
            ManifestCapabilityPolicy, ManifestDependencyKind, ProjectDependency,
            ProjectDependencySource,
        },
        dependency_cache::{
            DependencyCacheComponentWorldForeignPackage, DependencyCacheEntry,
            DependencyCacheTransitivePackage,
        },
        plugin_sources,
        shared::dependency::DependencyResolver,
    };

    use super::load_or_refresh_cache_entry;

    #[derive(Clone)]
    struct MockResolver {
        calls: Arc<Mutex<Vec<(PathBuf, String, bool)>>>,
        response: DependencyCacheEntry,
    }

    #[async_trait]
    impl DependencyResolver for MockResolver {
        fn resolve_manifest_dependencies_from_lock(
            &self,
            _project_root: &Path,
            _dependencies: &[ProjectDependency],
        ) -> anyhow::Result<Vec<crate::commands::build::ManifestDependency>> {
            Ok(Vec::new())
        }

        fn resolve_dependency_component_sources(
            &self,
            _project_root: &Path,
            _dependencies: &[crate::commands::build::ManifestDependency],
        ) -> anyhow::Result<BTreeMap<String, PathBuf>> {
            Ok(BTreeMap::new())
        }

        async fn load_or_refresh_cache_entry(
            &self,
            project_root: &Path,
            dependency: &ProjectDependency,
            namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
        ) -> anyhow::Result<DependencyCacheEntry> {
            self.calls.lock().expect("calls lock").push((
                project_root.to_path_buf(),
                dependency.name.clone(),
                namespace_registries.is_some(),
            ));
            Ok(self.response.clone())
        }
    }

    fn sample_dependency() -> ProjectDependency {
        ProjectDependency {
            name: "path-source-0".to_string(),
            version: "0.1.0".to_string(),
            kind: ManifestDependencyKind::Native,
            wit: ProjectDependencySource {
                source_kind: plugin_sources::SourceKind::Path,
                source: "registry/example".to_string(),
                registry: None,
                sha256: None,
            },
            requires: vec![],
            component: None,
            capabilities: ManifestCapabilityPolicy::default(),
        }
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
            wit_digest: "digest".to_string(),
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![DependencyCacheComponentWorldForeignPackage {
                name: "wasi:io".to_string(),
                version: Some("0.2.0".to_string()),
                interfaces: vec!["streams".to_string()],
                interfaces_recorded: true,
            }],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![DependencyCacheTransitivePackage {
                name: "wasi:cli".to_string(),
                registry: None,
                requirement: "^0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: format!("sha256:{}", "a".repeat(64)),
                source: None,
                path: "wit/deps/wasi-cli-0.2.0".to_string(),
            }],
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_or_refresh_cache_entry_delegates_to_resolver_with_same_arguments() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let resolver = MockResolver {
            calls: Arc::clone(&calls),
            response: sample_entry(),
        };
        let project_root = std::env::temp_dir().join(format!(
            "imago-cli-update-cache-tests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        let dependency = sample_dependency();
        let mut registries = plugin_sources::NamespaceRegistries::new();
        registries.insert("wasi".to_string(), "wasi.dev".to_string());

        let entry =
            load_or_refresh_cache_entry(&resolver, &project_root, &dependency, Some(&registries))
                .await
                .expect("delegation should succeed");

        assert_eq!(entry.name, dependency.name);
        let calls = calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, project_root);
        assert_eq!(calls[0].1, dependency.name);
        assert!(calls[0].2);
    }
}
