use std::path::{Component, Path};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};

use crate::commands::{
    build::{ManifestDependencyKind, ProjectDependency},
    plugin_sources,
};

use super::hydrate::dependency_wit_path;

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

pub(super) fn dependency_kind_label(kind: ManifestDependencyKind) -> &'static str {
    match kind {
        ManifestDependencyKind::Native => "native",
        ManifestDependencyKind::Wasm => "wasm",
    }
}

pub(super) fn validate_safe_wit_path(path: &str, field_name: &str) -> anyhow::Result<()> {
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

pub(super) fn parse_prefixed_sha256<'a>(
    value: &'a str,
    field_name: &str,
) -> anyhow::Result<&'a str> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(anyhow!("{field_name} must start with 'sha256:'"));
    };
    plugin_sources::validate_sha256_hex(hex, field_name)?;
    Ok(hex)
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

    use super::{
        DependencyCacheEntry, dependency_kind_label, parse_prefixed_sha256, validate_safe_wit_path,
    };

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
                sha256: Some("a".repeat(64)),
            }),
            capabilities: ManifestCapabilityPolicy::default(),
        }
    }

    fn sample_entry() -> DependencyCacheEntry {
        let dependency = sample_dependency();
        DependencyCacheEntry {
            name: dependency.name.clone(),
            resolved_package_name: None,
            version: dependency.version.clone(),
            kind: "wasm".to_string(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: "wit/deps/path-source-0-0.1.0".to_string(),
            wit_digest: "digest".to_string(),
            wit_source_fingerprint: None,
            component_source: dependency.component.as_ref().map(|v| v.source.clone()),
            component_registry: None,
            component_sha256: dependency.component.as_ref().and_then(|v| v.sha256.clone()),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        }
    }

    #[test]
    fn validate_safe_wit_path_rejects_paths_outside_wit_deps() {
        let err = validate_safe_wit_path("wit/cache/pkg", "wit_path")
            .expect_err("path outside wit/deps should fail");
        assert!(err.to_string().contains("must start with 'wit/deps'"));
    }

    #[test]
    fn parse_prefixed_sha256_rejects_missing_prefix_and_invalid_length() {
        let prefix_err =
            parse_prefixed_sha256("abc", "field").expect_err("missing prefix should fail");
        assert!(prefix_err.to_string().contains("must start with 'sha256:'"));

        let length_err =
            parse_prefixed_sha256("sha256:abc", "field").expect_err("short digest should fail");
        assert!(length_err.to_string().contains("64-character hex"));
    }

    #[test]
    fn dependency_kind_label_matches_manifest_kind() {
        assert_eq!(
            dependency_kind_label(ManifestDependencyKind::Native),
            "native"
        );
        assert_eq!(dependency_kind_label(ManifestDependencyKind::Wasm), "wasm");
    }

    #[test]
    fn validate_for_dependency_rejects_component_source_mismatch() {
        let dependency = sample_dependency();
        let mut entry = sample_entry();
        entry.component_source = Some("registry/other-component.wasm".to_string());

        let err = entry
            .validate_for_dependency(&dependency, None)
            .expect_err("component source mismatch should fail");
        assert!(err.to_string().contains("component source mismatch"));
    }
}
