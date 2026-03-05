use super::*;

pub(in crate::commands::build) fn parse_string_table(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };

    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("{} must be a table", field_name))?;

    let mut map = BTreeMap::new();
    for (key, value) in table {
        let text = value
            .as_str()
            .ok_or_else(|| anyhow!("{}.{} must be a string", field_name, key))?;
        map.insert(key.clone(), text.to_string());
    }

    Ok(map)
}

pub(crate) fn parse_namespace_registries(
    value: Option<&TomlValue>,
) -> anyhow::Result<plugin_sources::NamespaceRegistries> {
    let raw = parse_string_table(value, "namespace_registries")?;
    let mut registries = plugin_sources::NamespaceRegistries::new();
    for (namespace, registry) in raw {
        let normalized_namespace = namespace.trim();
        if normalized_namespace.is_empty() {
            return Err(anyhow!(
                "namespace_registries contains an empty namespace key"
            ));
        }
        let normalized_registry = plugin_sources::normalize_registry_name(&registry)
            .with_context(|| format!("namespace_registries.{namespace}"))?;
        if registries
            .insert(normalized_namespace.to_string(), normalized_registry)
            .is_some()
        {
            return Err(anyhow!(
                "namespace_registries contains duplicate namespace key after trimming: {normalized_namespace}"
            ));
        }
    }
    Ok(registries)
}

pub(crate) fn load_namespace_registries(
    project_root: &Path,
) -> anyhow::Result<plugin_sources::NamespaceRegistries> {
    let root = load_resolved_toml(project_root, true)?;
    parse_namespace_registries(root.get("namespace_registries"))
}

pub(crate) fn load_project_dependencies_with_namespace_registries(
    project_root: &Path,
    namespace_registries: &plugin_sources::NamespaceRegistries,
) -> anyhow::Result<Vec<ProjectDependency>> {
    let root = load_resolved_toml(project_root, true)?;
    parse_project_dependencies(root.get("dependencies"), Some(namespace_registries))
}

pub(crate) fn load_project_binding_sources_with_namespace_registries(
    project_root: &Path,
    namespace_registries: &plugin_sources::NamespaceRegistries,
) -> anyhow::Result<Vec<ProjectBindingSource>> {
    let root = load_resolved_toml(project_root, true)?;
    parse_project_binding_sources(root.get("bindings"), Some(namespace_registries))
}

pub(in crate::commands::build) fn parse_project_dependencies(
    value: Option<&TomlValue>,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<Vec<ProjectDependency>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("dependencies must be an array"))?;
    let mut dependencies = Vec::with_capacity(array.len());
    let mut names = BTreeSet::new();

    for (index, item) in array.iter().enumerate() {
        let table = item
            .as_table()
            .ok_or_else(|| anyhow!("dependencies[{index}] must be a table"))?;
        for key in table.keys() {
            if key == "name" {
                return Err(anyhow!(
                    "dependencies[{index}].name is no longer supported; dependency ID is resolved from source"
                ));
            }
            if !matches!(
                key.as_str(),
                "version"
                    | "kind"
                    | "wit"
                    | "oci"
                    | "path"
                    | "sha256"
                    | "registry"
                    | "requires"
                    | "component"
                    | "capabilities"
            ) {
                return Err(anyhow!("dependencies[{index}].{key} is not supported"));
            }
        }

        let version = table
            .get("version")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("dependencies[{index}].version must be a string"))?
            .trim()
            .to_string();
        if version.is_empty() {
            return Err(anyhow!("dependencies[{index}].version must not be empty"));
        }

        let kind = match table
            .get("kind")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("dependencies[{index}].kind must be a string"))?
            .trim()
        {
            "native" => ManifestDependencyKind::Native,
            "wasm" => ManifestDependencyKind::Wasm,
            other => {
                return Err(anyhow!(
                    "dependencies[{index}].kind must be one of: native, wasm (got: {other})"
                ));
            }
        };

        let wit = parse_dependency_wit_source(table, index, &version, namespace_registries)
            .with_context(|| {
                format!("failed to parse dependencies[{index}] source configuration")
            })?;
        let name = dependency_name_hint_from_source(index, &wit)
            .with_context(|| format!("failed to derive dependency id for dependencies[{index}]"))?;
        if !names.insert(name.clone()) {
            return Err(anyhow!(
                "dependencies resolves duplicate dependency id: {name}"
            ));
        }

        let requires = match table.get("requires") {
            None => Vec::new(),
            Some(value) => {
                let array = value
                    .as_array()
                    .ok_or_else(|| anyhow!("dependencies[{index}].requires must be an array"))?;
                let mut values = Vec::with_capacity(array.len());
                for (req_index, req) in array.iter().enumerate() {
                    let req = req
                        .as_str()
                        .ok_or_else(|| {
                            anyhow!("dependencies[{index}].requires[{req_index}] must be a string")
                        })?
                        .trim()
                        .to_string();
                    if req.is_empty() {
                        return Err(anyhow!(
                            "dependencies[{index}].requires[{req_index}] must not be empty"
                        ));
                    }
                    validate_dependency_package_name(&req).map_err(|err| {
                        anyhow!("dependencies[{index}].requires[{req_index}] is invalid: {err}")
                    })?;
                    values.push(req);
                }
                normalize_string_list(values)
            }
        };

        let capabilities = parse_capability_policy(
            table.get("capabilities"),
            &format!("dependencies[{index}].capabilities"),
        )?;

        let component = match table.get("component") {
            None => None,
            Some(value) => {
                let component_table = value
                    .as_table()
                    .ok_or_else(|| anyhow!("dependencies[{index}].component must be a table"))?;
                for key in component_table.keys() {
                    if !matches!(key.as_str(), "wit" | "oci" | "path" | "registry" | "sha256") {
                        return Err(anyhow!(
                            "dependencies[{index}].component.{key} is not supported"
                        ));
                    }
                }
                let source = parse_source_selector(
                    component_table,
                    &format!("dependencies[{index}].component"),
                    namespace_registries,
                )?;

                Some(ProjectDependencyComponent {
                    source_kind: source.source_kind,
                    source: source.source,
                    registry: source.registry,
                    sha256: source.sha256,
                })
            }
        };

        match kind {
            ManifestDependencyKind::Native => {
                if component.is_some() {
                    return Err(anyhow!(
                        "dependencies[{index}].component is only allowed when kind=\"wasm\""
                    ));
                }
            }
            ManifestDependencyKind::Wasm => {}
        }

        dependencies.push(ProjectDependency {
            name,
            version,
            kind,
            wit,
            requires,
            component,
            capabilities,
        });
    }

    Ok(dependencies)
}

pub(in crate::commands::build) fn parse_dependency_wit_source(
    table: &toml::Table,
    index: usize,
    version: &str,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<ProjectDependencySource> {
    if version.trim().is_empty() {
        return Err(anyhow!("dependencies[{index}].version must not be empty"));
    }
    parse_source_selector(
        table,
        &format!("dependencies[{index}]"),
        namespace_registries,
    )
}

pub(in crate::commands::build) fn dependency_name_hint_from_source(
    index: usize,
    source: &ProjectDependencySource,
) -> anyhow::Result<String> {
    match source.source_kind {
        plugin_sources::SourceKind::Wit => Ok(plugin_sources::parse_wit_package_source(
            &source.source,
            "dependencies[].wit",
        )?
        .to_string()),
        plugin_sources::SourceKind::Oci => {
            plugin_sources::parse_oci_package_source(&source.source, "dependencies[].oci")
        }
        plugin_sources::SourceKind::Path => Ok(format!("path-source-{index}")),
    }
}

pub(in crate::commands::build) fn parse_source_selector(
    table: &toml::Table,
    field_base: &str,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<ProjectDependencySource> {
    let wit = table.get("wit");
    let oci = table.get("oci");
    let path = table.get("path");
    let selected = [("wit", wit), ("oci", oci), ("path", path)]
        .into_iter()
        .filter(|(_, value)| value.is_some())
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(anyhow!(
            "{field_base} must define exactly one source key: `wit`, `oci`, or `path`"
        ));
    }
    if selected.len() > 1 {
        return Err(anyhow!(
            "{field_base} has multiple source keys; choose exactly one of `wit`, `oci`, or `path`"
        ));
    }
    let (kind_key, value) = selected[0];
    let source = value
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("{field_base}.{kind_key} must be a string"))?
        .trim()
        .to_string();
    if source.is_empty() {
        return Err(anyhow!("{field_base}.{kind_key} must not be empty"));
    }

    let source_kind = match kind_key {
        "wit" => plugin_sources::SourceKind::Wit,
        "oci" => plugin_sources::SourceKind::Oci,
        "path" => plugin_sources::SourceKind::Path,
        _ => unreachable!(),
    };
    plugin_sources::validate_wit_source(source_kind, &source, &format!("{field_base}.{kind_key}"))?;

    let raw_registry = match table.get("registry") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| anyhow!("{field_base}.registry must be a string"))?
                .trim()
                .to_string(),
        ),
    };
    let registry = plugin_sources::normalize_registry_for_source(
        source_kind,
        &source,
        raw_registry.as_deref(),
        namespace_registries,
        field_base,
    )?;
    let sha256 = match table.get("sha256") {
        None => None,
        Some(value) => {
            let sha = value
                .as_str()
                .ok_or_else(|| anyhow!("{field_base}.sha256 must be a string"))?
                .trim()
                .to_string();
            if sha.is_empty() {
                return Err(anyhow!("{field_base}.sha256 must not be empty"));
            }
            plugin_sources::validate_sha256_hex(&sha, &format!("{field_base}.sha256"))?;
            Some(sha)
        }
    };
    Ok(ProjectDependencySource {
        source_kind,
        source,
        registry,
        sha256,
    })
}

pub(in crate::commands::build) fn parse_project_binding_sources(
    value: Option<&TomlValue>,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<Vec<ProjectBindingSource>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("bindings must be an array"))?;
    let mut bindings = Vec::with_capacity(array.len());
    let mut wit_to_name = BTreeMap::<(String, Option<String>, String), String>::new();
    let mut seen_bindings = BTreeSet::<(String, String, Option<String>, String)>::new();
    for (index, entry) in array.iter().enumerate() {
        let table = entry
            .as_table()
            .ok_or_else(|| anyhow!("bindings[{index}] must be a table"))?;
        for key in table.keys() {
            if key == "target" {
                return Err(anyhow!(
                    "bindings[{index}].target is no longer supported; use bindings[{index}].name"
                ));
            }
            if !matches!(
                key.as_str(),
                "name" | "version" | "wit" | "oci" | "path" | "sha256" | "registry"
            ) {
                return Err(anyhow!("bindings[{index}].{key} is not supported"));
            }
        }
        let name = table
            .get("name")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("bindings[{index}].name must be a string"))?
            .trim()
            .to_string();
        let wit_version = table
            .get("version")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("bindings[{index}].version must be a string"))?
            .trim()
            .to_string();

        if name.is_empty() {
            return Err(anyhow!("bindings[{index}].name must not be empty"));
        }
        if wit_version.is_empty() {
            return Err(anyhow!("bindings[{index}].version must not be empty"));
        }
        validate_service_name(&name).map_err(|e| {
            anyhow!(
                "bindings[{index}].name is invalid: {}",
                e.to_string().replace("name ", "")
            )
        })?;
        let source =
            parse_source_selector(table, &format!("bindings[{index}]"), namespace_registries)?;
        let wit_source = source.source;
        let wit_registry = source.registry;
        let wit_source_kind = source.source_kind;
        let wit_sha256 = source.sha256;

        let source_key = (
            wit_source.clone(),
            wit_registry.clone(),
            wit_version.clone(),
        );
        if let Some(existing_name) = wit_to_name.get(&source_key) {
            if existing_name != &name {
                return Err(anyhow!(
                    "bindings wit '{}' maps to multiple services ('{}' and '{}'); this is ambiguous",
                    wit_source,
                    existing_name,
                    name
                ));
            }
        } else {
            wit_to_name.insert(source_key, name.clone());
        }

        if seen_bindings.insert((
            name.clone(),
            wit_source.clone(),
            wit_registry.clone(),
            wit_version.clone(),
        )) {
            bindings.push(ProjectBindingSource {
                name,
                wit_source_kind,
                wit_source,
                wit_registry,
                wit_version,
                wit_sha256,
            });
        }
    }
    Ok(bindings)
}

pub(in crate::commands::build) fn resolve_manifest_bindings_from_lock(
    project_root: &Path,
    bindings: &[ProjectBindingSource],
) -> anyhow::Result<Vec<ManifestBinding>> {
    if bindings.is_empty() {
        return Ok(Vec::new());
    }

    let lock = crate::lockfile::load_from_project_root(project_root)?;
    let mut expectations = Vec::with_capacity(bindings.len());
    for binding in bindings {
        expectations.push(binding_expectation_for_project_binding(binding));
    }
    let resolved = crate::lockfile::resolve_binding_wits(project_root, &lock, &expectations)?;

    let mut expanded = BTreeSet::<(String, String)>::new();
    for binding in resolved {
        for interface_id in binding.interfaces {
            expanded.insert((binding.name.clone(), interface_id));
        }
    }
    Ok(expanded
        .into_iter()
        .map(|(name, wit)| ManifestBinding { name, wit })
        .collect())
}

pub(in crate::commands::build) fn binding_expectation_for_project_binding(
    binding: &ProjectBindingSource,
) -> BindingWitExpectation {
    BindingWitExpectation {
        name: binding.name.clone(),
        source_kind: lock_source_kind(binding.wit_source_kind),
        source: binding.wit_source.clone(),
        registry: binding.wit_registry.clone(),
        version: binding.wit_version.clone(),
        sha256: binding.wit_sha256.clone(),
    }
}

pub(in crate::commands::build) fn lock_source_kind(
    kind: plugin_sources::SourceKind,
) -> LockSourceKind {
    match kind {
        plugin_sources::SourceKind::Wit => LockSourceKind::Wit,
        plugin_sources::SourceKind::Oci => LockSourceKind::Oci,
        plugin_sources::SourceKind::Path => LockSourceKind::Path,
    }
}

#[cfg(test)]
mod tests {
    use toml::Value as TomlValue;

    use super::{
        dependency_name_hint_from_source, parse_project_binding_sources, parse_source_selector,
    };
    use crate::commands::{build::ProjectDependencySource, plugin_sources};

    fn parse_table(raw: &str) -> toml::Table {
        toml::from_str::<TomlValue>(raw)
            .expect("toml should parse")
            .as_table()
            .expect("toml value should be a table")
            .clone()
    }

    #[test]
    fn parse_source_selector_rejects_multiple_source_keys() {
        let table = parse_table(
            r#"
wit = "acme:test"
path = "registry/example"
"#,
        );

        let err = parse_source_selector(&table, "dependencies[0]", None)
            .expect_err("multiple source keys must fail");
        assert!(err.to_string().contains("multiple source keys"));
    }

    #[test]
    fn parse_source_selector_accepts_single_path_source() {
        let table = parse_table(r#"path = "registry/example""#);
        let source = parse_source_selector(&table, "dependencies[0]", None)
            .expect("path source should pass");

        assert_eq!(source.source_kind, plugin_sources::SourceKind::Path);
        assert_eq!(source.source, "registry/example");
        assert!(source.registry.is_none());
        assert!(source.sha256.is_none());
    }

    #[test]
    fn dependency_name_hint_for_path_source_uses_index_prefix() {
        let source = ProjectDependencySource {
            source_kind: plugin_sources::SourceKind::Path,
            source: "registry/example".to_string(),
            registry: None,
            sha256: None,
        };

        let name = dependency_name_hint_from_source(3, &source)
            .expect("path source dependency id should be derived");
        assert_eq!(name, "path-source-3");
    }

    #[test]
    fn parse_project_binding_sources_rejects_ambiguous_source_to_service_mapping() {
        let bindings = toml::from_str::<TomlValue>(
            r#"
bindings = [
  { name = "service-a", version = "0.1.0", path = "registry/example" },
  { name = "service-b", version = "0.1.0", path = "registry/example" }
]
"#,
        )
        .expect("bindings toml should parse");

        let err = parse_project_binding_sources(bindings.get("bindings"), None)
            .expect_err("same wit source mapped to different service names must fail");
        assert!(err.to_string().contains("maps to multiple services"));
    }
}
