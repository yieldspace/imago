mod digest;
mod hydrate;
mod io;
mod model;

pub(crate) use hydrate::{
    component_source_fingerprint_if_exists, dependency_wit_path, dependency_wit_target_rel,
    hydrate_project_wit_deps, is_cache_hit, resolve_cached_component_path,
    verify_project_dependency_cache, wit_source_fingerprint_if_exists,
};
pub(crate) use io::{cache_component_path, cache_entry_root, load_entry, save_entry};
pub(crate) use model::{
    DependencyCacheComponentWorldForeignPackage, DependencyCacheEntry,
    DependencyCacheTransitivePackage,
};

pub(super) const CACHE_ROOT_REL: &str = ".imago/deps";
pub(super) const MISSING_CACHE_HINT: &str = "run `imago deps sync`";

#[cfg(test)]
mod tests {
    use crate::commands::{
        build::{
            ManifestCapabilityPolicy, ManifestDependencyKind, ProjectDependency,
            ProjectDependencyComponent, ProjectDependencySource,
        },
        plugin_sources,
    };

    use super::{
        DependencyCacheComponentWorldForeignPackage, DependencyCacheEntry,
        DependencyCacheTransitivePackage, dependency_wit_path,
    };

    fn sample_dependency() -> ProjectDependency {
        ProjectDependency {
            name: "example:dep".to_string(),
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
                sha256: Some("a".repeat(64)),
            }),
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    fn sample_cache_entry() -> DependencyCacheEntry {
        let dependency = sample_dependency();
        DependencyCacheEntry {
            name: dependency.name.clone(),
            resolved_package_name: None,
            version: dependency.version.clone(),
            kind: "wasm".to_string(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: dependency.wit.registry.clone(),
            wit_sha256: dependency.wit.sha256.clone(),
            wit_path: dependency_wit_path(&dependency.name, &dependency.version),
            wit_digest: "sha256-placeholder".to_string(),
            wit_source_fingerprint: None,
            component_source: dependency.component.as_ref().map(|v| v.source.clone()),
            component_registry: dependency
                .component
                .as_ref()
                .and_then(|v| v.registry.clone()),
            component_sha256: dependency.component.as_ref().and_then(|v| v.sha256.clone()),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![DependencyCacheComponentWorldForeignPackage {
                name: "wasi:io".to_string(),
                version: Some("0.2.0".to_string()),
                interfaces: vec!["streams".to_string()],
                interfaces_recorded: true,
            }],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        }
    }

    #[test]
    fn validate_for_dependency_accepts_matching_entry() {
        let dependency = sample_dependency();
        let entry = sample_cache_entry();

        entry
            .validate_for_dependency(&dependency, None)
            .expect("matching cache entry should pass");
    }

    #[test]
    fn validate_for_dependency_rejects_name_mismatch() {
        let dependency = sample_dependency();
        let mut entry = sample_cache_entry();
        entry.name = "other:dep".to_string();

        let err = entry
            .validate_for_dependency(&dependency, None)
            .expect_err("name mismatch must fail");
        assert!(err.to_string().contains("cache name mismatch"));
    }

    #[test]
    fn validate_for_dependency_rejects_missing_component_sha_for_wasm() {
        let dependency = sample_dependency();
        let mut entry = sample_cache_entry();
        entry.component_sha256 = None;

        let err = entry
            .validate_for_dependency(&dependency, None)
            .expect_err("missing wasm component sha must fail");
        assert!(err.to_string().contains("component sha256 is missing"));
    }

    #[test]
    fn validate_for_dependency_rejects_component_source_mismatch() {
        let dependency = sample_dependency();
        let mut entry = sample_cache_entry();
        entry.component_source = Some("registry/other-component.wasm".to_string());

        let err = entry
            .validate_for_dependency(&dependency, None)
            .expect_err("component source mismatch must fail");
        assert!(err.to_string().contains("component source mismatch"));
    }

    #[test]
    fn validate_for_dependency_rejects_empty_transitive_requirement() {
        let dependency = sample_dependency();
        let mut entry = sample_cache_entry();
        entry
            .transitive_packages
            .push(DependencyCacheTransitivePackage {
                name: "wasi:cli".to_string(),
                registry: Some("wasi.dev".to_string()),
                requirement: "   ".to_string(),
                version: Some("0.2.0".to_string()),
                digest: format!("sha256:{}", "b".repeat(64)),
                source: None,
                path: "wit/deps/wasi-cli-0.2.0".to_string(),
            });

        let err = entry
            .validate_for_dependency(&dependency, None)
            .expect_err("empty transitive requirement must fail");
        assert!(err.to_string().contains("transitive requirement is empty"));
    }
}
