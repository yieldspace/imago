use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, anyhow};
use dotenvy::from_path_iter;
use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::{
    commands::plugin_sources,
    lockfile::{BindingWitExpectation, LockSourceKind},
};

use super::{
    AssetSource, DEFAULT_HTTP_MAX_BODY_BYTES, MAX_HTTP_MAX_BODY_BYTES, ManifestAsset,
    ManifestBinding, ManifestCapabilityPolicy, ManifestDependencyKind, ManifestHttp,
    ManifestResourcesConfig, ManifestSocket, ManifestSocketDirection, ManifestSocketProtocol,
    ManifestWasiMount, ProjectBindingSource, ProjectDependency, ProjectDependencyComponent,
    ProjectDependencySource, TargetConfig, load_resolved_toml, validation as build_validation,
};

mod capabilities;
mod dependency;
mod resources;
mod target;
mod validation;

pub(in crate::commands::build) use capabilities::*;
pub(in crate::commands::build) use dependency::*;
pub(crate) use dependency::{
    load_namespace_registries, load_project_binding_sources_with_namespace_registries,
    load_project_dependencies_with_namespace_registries, parse_namespace_registries,
};
pub(in crate::commands::build) use resources::*;
pub(crate) use resources::{
    EmbeddedResourceProviderCandidate, LoadedResourceProvider, load_declared_resource_providers,
    load_embedded_resource_providers, load_resource_profile_expectations_from_providers,
};
pub(in crate::commands::build) use target::*;
pub(in crate::commands::build) use validation::*;
pub(crate) use validation::{validate_app_type, validate_service_name};

#[cfg(test)]
mod tests {
    use toml::Value as TomlValue;

    use super::{normalize_relative_path, parse_root_capabilities, parse_source_selector};
    use crate::commands::plugin_sources;

    fn parse_table(raw: &str) -> toml::Table {
        toml::from_str::<TomlValue>(raw)
            .expect("toml should parse")
            .as_table()
            .expect("value should be table")
            .clone()
    }

    #[test]
    fn config_parse_module_reexports_validation_helpers() {
        let normalized =
            normalize_relative_path("assets/logo.svg", "assets.path").expect("path should parse");
        assert_eq!(normalized, std::path::PathBuf::from("assets/logo.svg"));
    }

    #[test]
    fn config_parse_module_reexports_dependency_selector_helpers() {
        let table = parse_table(r#"path = "registry/example""#);
        let source =
            parse_source_selector(&table, "dependencies[0]", None).expect("source should parse");
        assert_eq!(source.source_kind, plugin_sources::SourceKind::Path);
        assert_eq!(source.source, "registry/example");
    }

    #[test]
    fn config_parse_module_reexports_capability_helpers() {
        let root = parse_table(
            r#"
[capabilities]
privileged = true
"#,
        );
        let capabilities = parse_root_capabilities(&root).expect("capabilities table should parse");
        assert!(capabilities.privileged);
    }
}
