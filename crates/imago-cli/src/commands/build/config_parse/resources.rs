use super::*;
use serde::Deserialize;
use sha2::Digest;
use wasmparser::{Parser, Payload};

pub(in crate::commands::build) fn parse_assets(
    value: Option<&TomlValue>,
    project_root: &Path,
) -> anyhow::Result<Vec<AssetSource>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("assets must be an array"))?;

    let mut assets = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let table = item
            .as_table()
            .ok_or_else(|| anyhow!("assets[{}] must be a table", index))?;

        let path_value = table
            .get("path")
            .ok_or_else(|| anyhow!("assets[{}].path is required", index))?;
        let path_text = path_value
            .as_str()
            .ok_or_else(|| anyhow!("assets[{}].path must be a string", index))?;
        let normalized = normalize_relative_path(path_text, "assets[].path")?;
        ensure_file_exists(project_root, &normalized, "assets[].path")?;

        let mut extra = BTreeMap::new();
        for (key, value) in table {
            if key == "path" {
                continue;
            }
            extra.insert(key.clone(), toml_to_json_normalized(value)?);
        }

        assets.push(AssetSource {
            manifest_asset: ManifestAsset {
                path: normalized_path_to_string(&normalized),
                extra,
            },
            source_path: normalized,
        });
    }

    Ok(assets)
}

pub(in crate::commands::build) fn parse_resources_section(
    root: &toml::Table,
    assets: &[AssetSource],
    _project_root: &Path,
) -> anyhow::Result<Option<ManifestResourcesConfig>> {
    let Some(value) = root.get("resources") else {
        return Ok(None);
    };
    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("resources must be a table"))?;

    let args = parse_resources_args(table.get("args"))?;
    let env = parse_string_table(table.get("env"), "resources.env")?;
    let http_outbound = parse_resources_http_outbound(table.get("http_outbound"))?;

    let allowed_asset_dirs = collect_allowed_resource_asset_dirs(assets);
    let mounts =
        parse_resource_mount_entries(table.get("mounts"), "resources.mounts", &allowed_asset_dirs)?;
    let read_only_mounts = parse_resource_mount_entries(
        table.get("read_only_mounts"),
        "resources.read_only_mounts",
        &allowed_asset_dirs,
    )?;
    validate_resource_mount_uniqueness(&mounts, &read_only_mounts)?;

    let mut extra = BTreeMap::new();
    for (key, value) in table {
        if matches!(
            key.as_str(),
            "args" | "env" | "http_outbound" | "mounts" | "read_only_mounts"
        ) {
            continue;
        }
        if key.trim().is_empty() {
            return Err(anyhow!("resources contains an empty key"));
        }
        let normalized = if key == "gpio" {
            parse_gpio_service_resource_value(value)?
        } else {
            toml_to_json_normalized(value)?
        };
        if key == "gpio" && normalized.is_null() {
            continue;
        }
        extra.insert(key.clone(), normalized);
    }

    let resources = ManifestResourcesConfig {
        args,
        env,
        http_outbound,
        mounts,
        read_only_mounts,
        extra,
    };
    if resources.is_empty() {
        Ok(None)
    } else {
        Ok(Some(resources))
    }
}

pub(crate) const IMAGO_RESOURCES_CUSTOM_SECTION_NAME: &str = "imago.resources.v1";
const GPIO_DIGITAL_PINS_PATH: &str = "gpio.digital_pins";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceMergePolicy {
    Sealed,
    Mergeable,
    Required,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedResourceProvider {
    pub expectation: crate::lockfile::ResourceProfileExpectation,
    pub provider_name: String,
    pub resources: JsonValue,
    pub merge_policies: BTreeMap<String, ResourceMergePolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GpioBoardProfileDocument {
    #[serde(default)]
    digital_pins: Vec<GpioBoardProfilePin>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GpioBoardProfilePin {
    soc_name: String,
    #[serde(default)]
    aliases: Vec<String>,
    gpio_num: u16,
    value_path: String,
    supports_input: bool,
    supports_output: bool,
    default_active_level: String,
    allow_pull_resistor: bool,
    header: String,
    physical_pin: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmbeddedResourceProviderDocument {
    schema_version: u32,
    resources: JsonValue,
    #[serde(default)]
    merge_policies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EmbeddedResourceProviderCandidate<'a> {
    pub project_dependency: &'a ProjectDependency,
    pub resolved_dependency_name: &'a str,
    pub component_sha256: &'a str,
}

pub(crate) fn load_declared_resource_providers(
    root: &toml::Table,
    project_root: &Path,
) -> anyhow::Result<Vec<LoadedResourceProvider>> {
    let Some(resources) = root.get("resources") else {
        return Ok(Vec::new());
    };
    let resources = resources
        .as_table()
        .ok_or_else(|| anyhow!("resources must be a table"))?;
    let Some(gpio) = resources.get("gpio") else {
        return Ok(Vec::new());
    };
    let gpio = gpio
        .as_table()
        .ok_or_else(|| anyhow!("resources.gpio must be a table"))?;
    let Some(profile) = gpio.get("profile") else {
        return Ok(Vec::new());
    };
    Ok(vec![load_gpio_profile_provider(profile, project_root)?])
}

#[cfg(test)]
pub(crate) fn load_resource_profile_expectations(
    root: &toml::Table,
    project_root: &Path,
) -> anyhow::Result<Vec<crate::lockfile::ResourceProfileExpectation>> {
    Ok(load_declared_resource_providers(root, project_root)?
        .into_iter()
        .map(|provider| provider.expectation)
        .collect())
}

fn parse_gpio_service_resource_value(value: &TomlValue) -> anyhow::Result<JsonValue> {
    let gpio = value
        .as_table()
        .ok_or_else(|| anyhow!("resources.gpio must be a table"))?;
    let mut keys = gpio.keys().cloned().collect::<Vec<_>>();
    keys.sort();

    let mut object = serde_json::Map::new();
    let mut has_profile = false;
    let mut has_digital_pins = false;
    for key in keys {
        let nested = gpio
            .get(&key)
            .ok_or_else(|| anyhow!("internal error: missing gpio resource key"))?;
        if key == "profile" {
            has_profile = true;
            continue;
        }
        if key == "digital_pins" {
            has_digital_pins = true;
        }
        object.insert(key, toml_to_json_normalized(nested)?);
    }

    if has_profile && !has_digital_pins && object.is_empty() {
        return Ok(JsonValue::Null);
    }

    Ok(JsonValue::Object(object))
}

fn load_gpio_profile_provider(
    value: &TomlValue,
    project_root: &Path,
) -> anyhow::Result<LoadedResourceProvider> {
    let profile = value
        .as_table()
        .ok_or_else(|| anyhow!("resources.gpio.profile must be a table"))?;
    let path_value = profile
        .get("path")
        .ok_or_else(|| anyhow!("resources.gpio.profile.path is required"))?;
    let path_text = path_value
        .as_str()
        .ok_or_else(|| anyhow!("resources.gpio.profile.path must be a string"))?;
    if profile.len() != 1 {
        return Err(anyhow!(
            "resources.gpio.profile currently supports only the path source"
        ));
    }

    let normalized_source = normalize_local_profile_path(path_text, "resources.gpio.profile.path")?;
    let source_path = if normalized_source.is_absolute() {
        normalized_source.clone()
    } else {
        project_root.join(&normalized_source)
    };
    let profile_bytes = std::fs::read(&source_path).with_context(|| {
        format!(
            "failed to read resources.gpio.profile.path '{}'",
            source_path.display()
        )
    })?;
    let profile_raw = String::from_utf8(profile_bytes.clone())
        .context("resources.gpio.profile.path must be valid UTF-8 TOML")?;
    let document: GpioBoardProfileDocument =
        toml::from_str(&profile_raw).context("failed to parse GPIO board profile")?;
    let digital_pins = gpio_profile_to_digital_pins_json(&document)?;
    let digest = hex::encode(sha2::Sha256::digest(&profile_bytes));

    Ok(LoadedResourceProvider {
        expectation: crate::lockfile::ResourceProfileExpectation {
            resource: "gpio".to_string(),
            profile_kind: "path".to_string(),
            source_kind: LockSourceKind::Path,
            source: normalized_source.to_string_lossy().into_owned(),
            provider_dependency: None,
            component_sha256: None,
            digest,
        },
        provider_name: normalized_source.to_string_lossy().into_owned(),
        resources: JsonValue::Object(serde_json::Map::from_iter([(
            "gpio".to_string(),
            JsonValue::Object(serde_json::Map::from_iter([(
                "digital_pins".to_string(),
                digital_pins,
            )])),
        )])),
        merge_policies: BTreeMap::from([(
            GPIO_DIGITAL_PINS_PATH.to_string(),
            ResourceMergePolicy::Mergeable,
        )]),
    })
}

pub(crate) fn load_embedded_resource_providers(
    project_root: &Path,
    candidates: &[EmbeddedResourceProviderCandidate<'_>],
) -> anyhow::Result<Vec<LoadedResourceProvider>> {
    let mut providers = Vec::new();
    for candidate in candidates {
        let component_path = crate::commands::dependency_cache::resolve_cached_component_path(
            project_root,
            &candidate.project_dependency.name,
            candidate.component_sha256,
        )?;
        let component_bytes = std::fs::read(&component_path).with_context(|| {
            format!(
                "failed to read cached component bytes for dependency '{}' from '{}'",
                candidate.resolved_dependency_name,
                component_path.display()
            )
        })?;
        let Some(section) =
            extract_embedded_resource_section(&component_bytes).with_context(|| {
                format!(
                    "failed to inspect embedded resources for dependency '{}'",
                    candidate.resolved_dependency_name
                )
            })?
        else {
            continue;
        };
        let metadata =
            parse_embedded_resource_provider_document(section, candidate.resolved_dependency_name)?;
        let (source_kind, source) = candidate
            .project_dependency
            .component
            .as_ref()
            .map(|component| {
                (
                    lock_source_kind_from_plugin_source(component.source_kind),
                    component.source.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    lock_source_kind_from_plugin_source(
                        candidate.project_dependency.wit.source_kind,
                    ),
                    candidate.project_dependency.wit.source.clone(),
                )
            });
        providers.push(LoadedResourceProvider {
            expectation: crate::lockfile::ResourceProfileExpectation {
                resource: "resources".to_string(),
                profile_kind: "component-custom-section".to_string(),
                source_kind,
                source,
                provider_dependency: Some(candidate.resolved_dependency_name.to_string()),
                component_sha256: Some(candidate.component_sha256.to_string()),
                digest: hex::encode(sha2::Sha256::digest(section)),
            },
            provider_name: candidate.resolved_dependency_name.to_string(),
            resources: metadata.resources,
            merge_policies: metadata.merge_policies,
        });
    }
    Ok(providers)
}

pub(crate) fn load_resource_profile_expectations_from_providers(
    providers: &[LoadedResourceProvider],
) -> Vec<crate::lockfile::ResourceProfileExpectation> {
    providers
        .iter()
        .map(|provider| provider.expectation.clone())
        .collect()
}

pub(in crate::commands::build) fn merge_resource_providers(
    service_resources: Option<ManifestResourcesConfig>,
    dotenv_resources_env: BTreeMap<String, String>,
    providers: &[LoadedResourceProvider],
) -> anyhow::Result<Option<ManifestResourcesConfig>> {
    let mut service_json = service_resources
        .map(serde_json::to_value)
        .transpose()
        .context("failed to convert service resources into merge input")?;
    if !dotenv_resources_env.is_empty() {
        let root = ensure_json_object(
            service_json.get_or_insert_with(|| JsonValue::Object(serde_json::Map::new())),
        )?;
        let env = match root
            .entry("env".to_string())
            .or_insert_with(|| JsonValue::Object(serde_json::Map::new()))
        {
            JsonValue::Object(env) => env,
            _ => {
                return Err(anyhow!(
                    "resources.env must be an object after merging dotenv resources"
                ));
            }
        };
        for (key, value) in dotenv_resources_env {
            env.insert(key, JsonValue::String(value));
        }
    }

    validate_declared_provider_conflicts(providers)?;

    let mut provider_json = JsonValue::Object(serde_json::Map::new());
    let mut policy_map = BTreeMap::new();
    for provider in providers {
        merge_provider_policy_map(
            &mut policy_map,
            &provider.merge_policies,
            &provider.provider_name,
        )?;
        merge_provider_value(
            &mut provider_json,
            &provider.resources,
            "",
            &provider.provider_name,
        )?;
    }

    let mut overridden_paths = BTreeSet::new();
    if let Some(service_json) = service_json.as_ref() {
        merge_service_value(
            &mut provider_json,
            service_json,
            "",
            false,
            &policy_map,
            &mut overridden_paths,
        )?;
    }
    ensure_required_service_overrides(&policy_map, &overridden_paths)?;
    validate_final_gpio_catalog(&provider_json)?;

    let resources: ManifestResourcesConfig =
        serde_json::from_value(provider_json).context("failed to materialize merged resources")?;
    if resources.is_empty() {
        Ok(None)
    } else {
        Ok(Some(resources))
    }
}

fn extract_embedded_resource_section(component_bytes: &[u8]) -> anyhow::Result<Option<&[u8]>> {
    let mut section = None;
    for payload in Parser::new(0).parse_all(component_bytes) {
        let payload: Payload<'_> = payload.context("failed to parse component bytes")?;
        if let Payload::CustomSection(custom) = payload
            && custom.name() == IMAGO_RESOURCES_CUSTOM_SECTION_NAME
            && section.replace(custom.data()).is_some()
        {
            return Err(anyhow!(
                "component defines duplicate custom section '{}'",
                IMAGO_RESOURCES_CUSTOM_SECTION_NAME
            ));
        }
    }
    Ok(section)
}

struct EmbeddedResourceProviderDocumentNormalized {
    resources: JsonValue,
    merge_policies: BTreeMap<String, ResourceMergePolicy>,
}

fn parse_embedded_resource_provider_document(
    section: &[u8],
    provider_name: &str,
) -> anyhow::Result<EmbeddedResourceProviderDocumentNormalized> {
    let raw = std::str::from_utf8(section).with_context(|| {
        format!(
            "embedded resource section '{}' for dependency '{}' must be valid UTF-8 JSON",
            IMAGO_RESOURCES_CUSTOM_SECTION_NAME, provider_name
        )
    })?;
    let document: EmbeddedResourceProviderDocument =
        serde_json::from_str(raw).with_context(|| {
            format!(
                "embedded resource section '{}' for dependency '{}' must be valid JSON",
                IMAGO_RESOURCES_CUSTOM_SECTION_NAME, provider_name
            )
        })?;
    if document.schema_version != 1 {
        return Err(anyhow!(
            "embedded resource section '{}' for dependency '{}' must use schema_version = 1",
            IMAGO_RESOURCES_CUSTOM_SECTION_NAME,
            provider_name
        ));
    }
    if !document.resources.is_object() {
        return Err(anyhow!(
            "embedded resource section '{}' for dependency '{}' must define resources as an object",
            IMAGO_RESOURCES_CUSTOM_SECTION_NAME,
            provider_name
        ));
    }

    let mut merge_policies = BTreeMap::new();
    for (path, raw_policy) in document.merge_policies {
        let normalized_path = normalize_policy_path(&path)?;
        let policy = parse_resource_merge_policy(&raw_policy).with_context(|| {
            format!(
                "embedded resource provider '{}' merge_policies.{}",
                provider_name, normalized_path
            )
        })?;
        merge_policies.insert(normalized_path, policy);
    }

    Ok(EmbeddedResourceProviderDocumentNormalized {
        resources: document.resources,
        merge_policies,
    })
}

fn normalize_policy_path(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim().trim_start_matches("resources.");
    if trimmed.is_empty() {
        return Err(anyhow!("merge policy path must not be empty"));
    }
    let segments = trimmed.split('.').collect::<Vec<_>>();
    if segments.iter().any(|segment| segment.trim().is_empty()) {
        return Err(anyhow!("merge policy path must not contain empty segments"));
    }
    Ok(segments.join("."))
}

fn parse_resource_merge_policy(raw: &str) -> anyhow::Result<ResourceMergePolicy> {
    match raw.trim() {
        "sealed" => Ok(ResourceMergePolicy::Sealed),
        "mergeable" => Ok(ResourceMergePolicy::Mergeable),
        "required" => Ok(ResourceMergePolicy::Required),
        other => Err(anyhow!(
            "merge policy must be one of: sealed, mergeable, required (got: {other})"
        )),
    }
}

fn lock_source_kind_from_plugin_source(source_kind: plugin_sources::SourceKind) -> LockSourceKind {
    match source_kind {
        plugin_sources::SourceKind::Wit => LockSourceKind::Wit,
        plugin_sources::SourceKind::Oci => LockSourceKind::Oci,
        plugin_sources::SourceKind::Path => LockSourceKind::Path,
    }
}

fn validate_declared_provider_conflicts(
    providers: &[LoadedResourceProvider],
) -> anyhow::Result<()> {
    let path_provider = providers
        .iter()
        .find(|provider| provider.expectation.profile_kind == "path");
    if let Some(path_provider) = path_provider {
        for provider in providers {
            if provider.provider_name == path_provider.provider_name {
                continue;
            }
            if resource_object_contains_path(&provider.resources, GPIO_DIGITAL_PINS_PATH) {
                return Err(anyhow!(
                    "resource provider '{}' conflicts with '{}' at resources.{}",
                    provider.provider_name,
                    path_provider.provider_name,
                    GPIO_DIGITAL_PINS_PATH
                ));
            }
        }
    }
    Ok(())
}

fn resource_object_contains_path(resources: &JsonValue, path: &str) -> bool {
    let mut current = resources;
    for segment in path.split('.') {
        let Some(next) = current.as_object().and_then(|object| object.get(segment)) else {
            return false;
        };
        current = next;
    }
    true
}

fn merge_provider_policy_map(
    merged: &mut BTreeMap<String, ResourceMergePolicy>,
    incoming: &BTreeMap<String, ResourceMergePolicy>,
    provider_name: &str,
) -> anyhow::Result<()> {
    for (path, policy) in incoming {
        if let Some(existing) = merged.insert(path.clone(), *policy)
            && existing != *policy
        {
            return Err(anyhow!(
                "resource provider '{}' defines conflicting merge policy for resources.{}",
                provider_name,
                path
            ));
        }
    }
    Ok(())
}

fn merge_provider_value(
    target: &mut JsonValue,
    incoming: &JsonValue,
    path: &str,
    provider_name: &str,
) -> anyhow::Result<()> {
    match (target, incoming) {
        (JsonValue::Object(target), JsonValue::Object(incoming)) => {
            for (key, incoming_value) in incoming {
                let child_path = join_resource_path(path, key);
                if let Some(target_value) = target.get_mut(key) {
                    merge_provider_value(target_value, incoming_value, &child_path, provider_name)?;
                } else {
                    target.insert(key.clone(), incoming_value.clone());
                }
            }
            Ok(())
        }
        (JsonValue::Array(target), JsonValue::Array(incoming))
            if path == GPIO_DIGITAL_PINS_PATH =>
        {
            merge_provider_gpio_digital_pins(target, incoming, provider_name)
        }
        _ => Err(anyhow!(
            "resource provider '{}' conflicts with another provider at resources.{}",
            provider_name,
            path
        )),
    }
}

fn merge_provider_gpio_digital_pins(
    target: &mut Vec<JsonValue>,
    incoming: &[JsonValue],
    provider_name: &str,
) -> anyhow::Result<()> {
    let mut seen_public_names = BTreeSet::new();
    for entry in target.iter() {
        for public_name in
            digital_pin_public_names(entry, "resources.gpio.digital_pins provider entry")?
        {
            seen_public_names.insert(public_name);
        }
    }
    for entry in incoming {
        for public_name in
            digital_pin_public_names(entry, "resources.gpio.digital_pins provider entry")?
        {
            if !seen_public_names.insert(public_name.clone()) {
                return Err(anyhow!(
                    "resource provider '{}' conflicts with another provider at resources.gpio.digital_pins[{public_name}]",
                    provider_name
                ));
            }
        }
        target.push(entry.clone());
    }
    Ok(())
}

fn merge_service_value(
    target: &mut JsonValue,
    incoming: &JsonValue,
    path: &str,
    provider_owned: bool,
    policies: &BTreeMap<String, ResourceMergePolicy>,
    overridden_paths: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    match (target, incoming) {
        (JsonValue::Object(target), JsonValue::Object(incoming)) => {
            for (key, incoming_value) in incoming {
                let child_path = join_resource_path(path, key);
                if let Some(target_value) = target.get_mut(key) {
                    merge_service_value(
                        target_value,
                        incoming_value,
                        &child_path,
                        true,
                        policies,
                        overridden_paths,
                    )?;
                } else if provider_owned {
                    ensure_service_policy_allows_override(policies, &child_path)?;
                    target.insert(key.clone(), incoming_value.clone());
                } else {
                    target.insert(key.clone(), incoming_value.clone());
                }
            }
            Ok(())
        }
        (JsonValue::Array(target), JsonValue::Array(incoming))
            if path == GPIO_DIGITAL_PINS_PATH && provider_owned =>
        {
            ensure_service_policy_allows_override(policies, path)?;
            merge_service_gpio_digital_pins(target, incoming, overridden_paths)
        }
        (target, incoming) => {
            if provider_owned {
                ensure_service_policy_allows_override(policies, path)?;
                if !path.is_empty() {
                    overridden_paths.insert(path.to_string());
                }
            }
            *target = incoming.clone();
            Ok(())
        }
    }
}

fn merge_service_gpio_digital_pins(
    target: &mut [JsonValue],
    incoming: &[JsonValue],
    overridden_paths: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    let mut label_to_index = BTreeMap::new();
    for (index, entry) in target.iter().enumerate() {
        let label = digital_pin_label(entry, "resources.gpio.digital_pins provider entry")?;
        label_to_index.insert(label, index);
    }
    let mut seen_patch_labels = BTreeSet::new();

    for (index, patch) in incoming.iter().enumerate() {
        let patch_object = patch
            .as_object()
            .ok_or_else(|| anyhow!("resources.gpio.digital_pins[{index}] must be a table"))?;
        let label = patch_object
            .get("label")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .ok_or_else(|| anyhow!("resources.gpio.digital_pins[{index}].label is required"))?
            .to_string();
        if !seen_patch_labels.insert(label.clone()) {
            return Err(anyhow!(
                "resources.gpio.digital_pins contains duplicate patch label: {label}"
            ));
        }
        let target_index = label_to_index.get(&label).copied().ok_or_else(|| {
            anyhow!("resources.gpio.digital_pins patch references unknown label: {label}")
        })?;
        let target_object = target[target_index].as_object_mut().ok_or_else(|| {
            anyhow!("resources.gpio.digital_pins[{label}] provider entry must stay a table")
        })?;
        for (field, value) in patch_object {
            if field == "label" {
                continue;
            }
            if target_object.contains_key(field) {
                overridden_paths.insert(format!("gpio.digital_pins[{label}].{field}"));
            }
            target_object.insert(field.clone(), value.clone());
        }
    }
    Ok(())
}

fn ensure_service_policy_allows_override(
    policies: &BTreeMap<String, ResourceMergePolicy>,
    path: &str,
) -> anyhow::Result<()> {
    match policy_for_path(policies, path) {
        ResourceMergePolicy::Mergeable | ResourceMergePolicy::Required => Ok(()),
        ResourceMergePolicy::Sealed => Err(anyhow!(
            "service resources must not override sealed provider path resources.{}",
            path
        )),
    }
}

fn policy_for_path(
    policies: &BTreeMap<String, ResourceMergePolicy>,
    path: &str,
) -> ResourceMergePolicy {
    let mut best: Option<(&String, ResourceMergePolicy)> = None;
    for (policy_path, policy) in policies {
        let is_more_specific = match best {
            Some((best_path, _)) => policy_path.len() > best_path.len(),
            None => true,
        };
        if path_matches_policy_path(path, policy_path) && is_more_specific {
            best = Some((policy_path, *policy));
        }
    }
    best.map(|(_, policy)| policy)
        .unwrap_or(ResourceMergePolicy::Sealed)
}

fn path_matches_policy_path(path: &str, policy_path: &str) -> bool {
    path == policy_path
        || path
            .strip_prefix(policy_path)
            .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('['))
}

fn ensure_required_service_overrides(
    policies: &BTreeMap<String, ResourceMergePolicy>,
    overridden_paths: &BTreeSet<String>,
) -> anyhow::Result<()> {
    for (path, policy) in policies {
        if *policy != ResourceMergePolicy::Required {
            continue;
        }
        let satisfied = overridden_paths
            .iter()
            .any(|touched| path_matches_policy_path(touched, path));
        if !satisfied {
            return Err(anyhow!(
                "service resources must override required provider path resources.{}",
                path
            ));
        }
    }
    Ok(())
}

fn validate_final_gpio_catalog(resources: &JsonValue) -> anyhow::Result<()> {
    let Some(gpio) = resources
        .as_object()
        .and_then(|resources| resources.get("gpio"))
    else {
        return Ok(());
    };
    let Some(digital_pins) = gpio.as_object().and_then(|gpio| gpio.get("digital_pins")) else {
        return Ok(());
    };
    let pins = digital_pins
        .as_array()
        .ok_or_else(|| anyhow!("resources.gpio.digital_pins must be an array"))?;
    let mut seen_public_names = BTreeSet::new();
    for (index, pin) in pins.iter().enumerate() {
        let pin_path = format!("resources.gpio.digital_pins[{index}]");
        let object = pin
            .as_object()
            .ok_or_else(|| anyhow!("{pin_path} must be a table"))?;
        for field in [
            "label",
            "value_path",
            "supports_input",
            "supports_output",
            "default_active_level",
            "allow_pull_resistor",
        ] {
            if !object.contains_key(field) {
                return Err(anyhow!(
                    "{pin_path}.{field} is required after resource merge"
                ));
            }
        }
        for public_name in digital_pin_public_names(pin, &pin_path)? {
            if !seen_public_names.insert(public_name.clone()) {
                return Err(anyhow!(
                    "resources.gpio.digital_pins contains duplicated public name: {public_name}"
                ));
            }
        }
    }
    Ok(())
}

fn ensure_json_object(
    value: &mut JsonValue,
) -> anyhow::Result<&mut serde_json::Map<String, JsonValue>> {
    value
        .as_object_mut()
        .ok_or_else(|| anyhow!("resources must be a JSON object"))
}

fn digital_pin_label(entry: &JsonValue, field_name: &str) -> anyhow::Result<String> {
    entry
        .as_object()
        .and_then(|entry| entry.get("label"))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("{field_name}.label is required"))
}

fn digital_pin_public_names(entry: &JsonValue, field_name: &str) -> anyhow::Result<Vec<String>> {
    let label = digital_pin_label(entry, field_name)?;
    let mut public_names = vec![label];

    let Some(entry) = entry.as_object() else {
        return Err(anyhow!("{field_name} must be a table"));
    };
    let Some(aliases) = entry.get("aliases") else {
        return Ok(public_names);
    };
    let aliases = aliases
        .as_array()
        .ok_or_else(|| anyhow!("{field_name}.aliases must be an array"))?;
    for (alias_index, alias) in aliases.iter().enumerate() {
        let alias = alias
            .as_str()
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .ok_or_else(|| anyhow!("{field_name}.aliases[{alias_index}] must be a string"))?;
        public_names.push(alias.to_string());
    }
    Ok(public_names)
}

fn join_resource_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

fn normalize_local_profile_path(raw: &str, field_name: &str) -> anyhow::Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    if trimmed.contains("://") {
        return Err(anyhow!(
            "{field_name} currently supports only local file paths"
        ));
    }
    if trimmed.contains('\\') {
        return Err(anyhow!("{field_name} must not contain backslashes"));
    }

    let path = Path::new(trimmed);
    let is_absolute = path.is_absolute();
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => {
                return Err(anyhow!("{field_name} must not use a Windows path prefix"));
            }
            Component::RootDir => {}
            Component::CurDir => {}
            Component::Normal(segment) => segments.push(segment.to_os_string()),
            Component::ParentDir => {
                if is_absolute {
                    if !segments.is_empty() {
                        segments.pop();
                    }
                } else if segments
                    .last()
                    .is_some_and(|segment| segment != std::ffi::OsStr::new(".."))
                {
                    segments.pop();
                } else {
                    segments.push("..".into());
                }
            }
        }
    }

    let mut normalized = if is_absolute {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    for segment in segments {
        normalized.push(segment);
    }
    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} must not resolve to an empty path"));
    }
    Ok(normalized)
}

fn gpio_profile_to_digital_pins_json(
    document: &GpioBoardProfileDocument,
) -> anyhow::Result<JsonValue> {
    if document.digital_pins.is_empty() {
        return Err(anyhow!(
            "GPIO board profile must define at least one [[digital_pins]] entry"
        ));
    }

    let mut seen_public_names = BTreeSet::new();
    let mut seen_value_paths = BTreeSet::new();
    let mut digital_pins = Vec::with_capacity(document.digital_pins.len());
    for (index, pin) in document.digital_pins.iter().enumerate() {
        let pin_path = format!("digital_pins[{index}]");
        let label = normalize_profile_pin_name(&pin.soc_name, &format!("{pin_path}.soc_name"))?;
        if !seen_public_names.insert(label.clone()) {
            return Err(anyhow!("{pin_path}.soc_name is duplicated: {label}"));
        }

        let aliases = pin
            .aliases
            .iter()
            .enumerate()
            .map(|(alias_index, alias)| {
                normalize_profile_pin_name(alias, &format!("{pin_path}.aliases[{alias_index}]"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        for alias in &aliases {
            if !seen_public_names.insert(alias.clone()) {
                return Err(anyhow!(
                    "{pin_path}.aliases contains duplicated public name: {alias}"
                ));
            }
        }

        if pin.gpio_num == 0 {
            return Err(anyhow!("{pin_path}.gpio_num must be greater than 0"));
        }
        let header = pin.header.trim();
        if header.is_empty() {
            return Err(anyhow!("{pin_path}.header must not be empty"));
        }
        if pin.physical_pin == 0 && !header.eq_ignore_ascii_case("onboard") {
            return Err(anyhow!(
                "{pin_path}.physical_pin = 0 is reserved for onboard resources"
            ));
        }
        if !pin.supports_input && !pin.supports_output {
            return Err(anyhow!(
                "{pin_path} must allow at least one mode (supports_input or supports_output)"
            ));
        }
        let value_path =
            normalize_profile_value_path(&pin.value_path, &format!("{pin_path}.value_path"))?;
        if !seen_value_paths.insert(value_path.clone()) {
            return Err(anyhow!("{pin_path}.value_path is duplicated: {value_path}"));
        }
        if pin.default_active_level != "active-high" && pin.default_active_level != "active-low" {
            return Err(anyhow!(
                "{pin_path}.default_active_level must be 'active-high' or 'active-low'"
            ));
        }

        let mut object = serde_json::Map::new();
        object.insert("label".to_string(), JsonValue::String(label));
        if !aliases.is_empty() {
            object.insert(
                "aliases".to_string(),
                JsonValue::Array(aliases.into_iter().map(JsonValue::String).collect()),
            );
        }
        object.insert("value_path".to_string(), JsonValue::String(value_path));
        object.insert(
            "supports_input".to_string(),
            JsonValue::Bool(pin.supports_input),
        );
        object.insert(
            "supports_output".to_string(),
            JsonValue::Bool(pin.supports_output),
        );
        object.insert(
            "default_active_level".to_string(),
            JsonValue::String(pin.default_active_level.clone()),
        );
        object.insert(
            "allow_pull_resistor".to_string(),
            JsonValue::Bool(pin.allow_pull_resistor),
        );
        digital_pins.push(JsonValue::Object(object));
    }

    Ok(JsonValue::Array(digital_pins))
}

fn normalize_profile_pin_name(raw: &str, field_name: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    Ok(trimmed.to_string())
}

fn normalize_profile_value_path(raw: &str, field_name: &str) -> anyhow::Result<String> {
    let path = Path::new(raw.trim());
    if path.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    if !path.is_absolute() {
        return Err(anyhow!("{field_name} must be an absolute path"));
    }
    if raw.contains('\\') {
        return Err(anyhow!("{field_name} must not contain backslashes"));
    }

    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::Prefix(_) => {
                return Err(anyhow!("{field_name} must not use a Windows path prefix"));
            }
            Component::RootDir | Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    if normalized.file_name() != Some(std::ffi::OsStr::new("value")) {
        return Err(anyhow!("{field_name} must target a GPIO value file"));
    }
    Ok(normalized.to_string_lossy().into_owned())
}

pub(in crate::commands::build) fn parse_resources_args(
    value: Option<&TomlValue>,
) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("resources.args must be an array of strings"))?;
    let mut args = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let arg = value
            .as_str()
            .ok_or_else(|| anyhow!("resources.args[{index}] must be a string"))?
            .trim()
            .to_string();
        if arg.is_empty() {
            return Err(anyhow!("resources.args[{index}] must not be empty"));
        }
        args.push(arg);
    }
    Ok(args)
}

pub(in crate::commands::build) fn parse_resources_http_outbound(
    value: Option<&TomlValue>,
) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("resources.http_outbound must be an array of strings"))?;
    let mut rules = Vec::with_capacity(array.len());
    let mut seen = BTreeSet::new();
    for (index, value) in array.iter().enumerate() {
        let raw = value
            .as_str()
            .ok_or_else(|| anyhow!("resources.http_outbound[{index}] must be a string"))?;
        let normalized =
            normalize_wasi_http_outbound_rule(raw, &format!("resources.http_outbound[{index}]"))?;
        if seen.insert(normalized.clone()) {
            rules.push(normalized);
        }
    }
    Ok(rules)
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_rule(
    raw: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    if value.contains('*') {
        return Err(anyhow!("{field_name} wildcard is not supported: {}", value));
    }
    if value.chars().any(|ch| ch.is_whitespace()) {
        return Err(anyhow!(
            "{field_name} must not contain whitespace: {}",
            value
        ));
    }
    if value.contains('/') {
        return normalize_wasi_http_outbound_cidr(value, field_name);
    }

    normalize_wasi_http_outbound_host_or_host_port(value, field_name)
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_cidr(
    value: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let (ip_text, prefix_text) = value.split_once('/').ok_or_else(|| {
        anyhow!(
            "{field_name} must be hostname, host:port, or CIDR: {}",
            value
        )
    })?;
    if ip_text.is_empty() || prefix_text.is_empty() || prefix_text.contains('/') {
        return Err(anyhow!(
            "{field_name} must be valid CIDR (<ip>/<prefix>): {}",
            value
        ));
    }
    let ip = ip_text.parse::<IpAddr>().map_err(|err| {
        anyhow!(
            "{field_name} CIDR ip is invalid '{}': {err}",
            ip_text.trim()
        )
    })?;
    let prefix = prefix_text.parse::<u8>().map_err(|err| {
        anyhow!(
            "{field_name} CIDR prefix is invalid '{}': {err}",
            prefix_text.trim()
        )
    })?;
    let max_prefix = match ip {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if prefix > max_prefix {
        return Err(anyhow!(
            "{field_name} CIDR prefix must be in range 0..={max_prefix}: {}",
            prefix
        ));
    }

    let network_ip = cidr_network_ip(ip, prefix);
    Ok(format!("{network_ip}/{prefix}"))
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_host_or_host_port(
    value: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    if value.starts_with('[') {
        let close_index = value
            .find(']')
            .ok_or_else(|| anyhow!("{field_name} has invalid bracketed host: {value}"))?;
        let host_text = &value[1..close_index];
        let host_ip = host_text.parse::<Ipv6Addr>().map_err(|err| {
            anyhow!(
                "{field_name} bracketed host must be valid IPv6: {} ({err})",
                host_text
            )
        })?;
        let rest = &value[(close_index + 1)..];
        if rest.is_empty() {
            return Ok(host_ip.to_string());
        }
        let port_text = rest.strip_prefix(':').ok_or_else(|| {
            anyhow!(
                "{field_name} bracketed host must use [ipv6]:port format: {}",
                value
            )
        })?;
        let port = parse_wasi_http_outbound_port(port_text, field_name)?;
        return Ok(format!("[{host_ip}]:{port}"));
    }

    if value.matches(':').count() > 1 {
        let ip = value.parse::<IpAddr>().map_err(|err| {
            anyhow!(
                "{field_name} must use [ipv6]:port for IPv6 host: {} ({err})",
                value
            )
        })?;
        return Ok(ip.to_string());
    }

    if let Some((host_text, port_text)) = value.rsplit_once(':')
        && port_text.chars().all(|ch| ch.is_ascii_digit())
    {
        let host = normalize_wasi_http_outbound_host(host_text, field_name)?;
        let port = parse_wasi_http_outbound_port(port_text, field_name)?;
        if host.contains(':') {
            return Ok(format!("[{host}]:{port}"));
        }
        return Ok(format!("{host}:{port}"));
    }

    normalize_wasi_http_outbound_host(value, field_name)
}

pub(in crate::commands::build) fn normalize_wasi_http_outbound_host(
    raw_host: &str,
    field_name: &str,
) -> anyhow::Result<String> {
    let host = raw_host.trim();
    if host.is_empty() {
        return Err(anyhow!("{field_name} host must not be empty"));
    }
    if host.contains('*') {
        return Err(anyhow!(
            "{field_name} wildcard host is not supported: {}",
            host
        ));
    }
    if host.contains('/') || host.contains('\\') {
        return Err(anyhow!(
            "{field_name} host must not contain path separators: {}",
            host
        ));
    }
    if host.chars().any(|ch| ch.is_whitespace()) {
        return Err(anyhow!(
            "{field_name} host must not contain whitespace: {}",
            host
        ));
    }
    if host.starts_with('[') || host.ends_with(']') {
        return Err(anyhow!(
            "{field_name} host must not contain brackets: {}",
            host
        ));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ip.to_string());
    }

    if host.contains(':') {
        return Err(anyhow!(
            "{field_name} host with ':' must use [ipv6]:port format: {}",
            host
        ));
    }

    Ok(host.to_ascii_lowercase())
}

pub(in crate::commands::build) fn parse_wasi_http_outbound_port(
    port_text: &str,
    field_name: &str,
) -> anyhow::Result<u16> {
    let port = port_text.parse::<u16>().map_err(|err| {
        anyhow!(
            "{field_name} port must be in range 1..=65535 (got '{}'): {err}",
            port_text
        )
    })?;
    if port == 0 {
        return Err(anyhow!(
            "{field_name} port must be in range 1..=65535 (got 0)"
        ));
    }
    Ok(port)
}

pub(in crate::commands::build) fn cidr_network_ip(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << u32::from(32_u8.saturating_sub(prefix))
            };
            IpAddr::V4(Ipv4Addr::from(bits & mask))
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << u32::from(128_u8.saturating_sub(prefix))
            };
            IpAddr::V6(Ipv6Addr::from(bits & mask))
        }
    }
}

pub(in crate::commands::build) fn load_dotenv_resources_env(
    project_root: &Path,
) -> anyhow::Result<BTreeMap<String, String>> {
    let path = project_root.join(".env");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let iter =
        from_path_iter(&path).with_context(|| format!("failed to parse {}", path.display()))?;

    let mut env = BTreeMap::new();
    for entry in iter {
        let (key, value) = entry.with_context(|| format!("failed to parse {}", path.display()))?;
        env.insert(key, value);
    }
    Ok(env)
}

pub(in crate::commands::build) fn collect_allowed_resource_asset_dirs(
    assets: &[AssetSource],
) -> BTreeSet<PathBuf> {
    let mut allowed = BTreeSet::new();
    for asset in assets {
        if let Some(parent) = asset.source_path.parent()
            && !parent.as_os_str().is_empty()
        {
            allowed.insert(parent.to_path_buf());
        }
    }
    allowed
}

pub(in crate::commands::build) fn parse_resource_mount_entries(
    value: Option<&TomlValue>,
    field_name: &str,
    allowed_asset_dirs: &BTreeSet<PathBuf>,
) -> anyhow::Result<Vec<ManifestWasiMount>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("{field_name} must be an array"))?;
    let mut mounts = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let entry = item
            .as_table()
            .ok_or_else(|| anyhow!("{field_name}[{index}] must be a table"))?;
        for key in entry.keys() {
            if !matches!(key.as_str(), "asset_dir" | "guest_path") {
                return Err(anyhow!("{field_name}[{index}].{key} is not supported"));
            }
        }

        let asset_dir_raw = entry
            .get("asset_dir")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("{field_name}[{index}].asset_dir must be a string"))?;
        let asset_dir =
            normalize_relative_path(asset_dir_raw, &format!("{field_name}[{index}].asset_dir"))?;
        if !allowed_asset_dirs.contains(&asset_dir) {
            return Err(anyhow!(
                "{field_name}[{index}].asset_dir must match a directory derived from assets[].path"
            ));
        }

        let guest_path_raw = entry
            .get("guest_path")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("{field_name}[{index}].guest_path must be a string"))?;
        let guest_path = normalize_wasi_guest_path(
            guest_path_raw,
            &format!("{field_name}[{index}].guest_path"),
        )?;

        mounts.push(ManifestWasiMount {
            asset_dir: normalized_path_to_string(&asset_dir),
            guest_path,
        });
    }
    Ok(mounts)
}

pub(in crate::commands::build) fn validate_resource_mount_uniqueness(
    mounts: &[ManifestWasiMount],
    read_only_mounts: &[ManifestWasiMount],
) -> anyhow::Result<()> {
    let mut seen_guest_paths = BTreeSet::new();
    let mut seen_asset_dirs = BTreeSet::new();
    for mount in mounts.iter().chain(read_only_mounts.iter()) {
        if !seen_guest_paths.insert(mount.guest_path.clone()) {
            return Err(anyhow!(
                "resources mounts contain duplicate guest_path: {}",
                mount.guest_path
            ));
        }
        if !seen_asset_dirs.insert(mount.asset_dir.clone()) {
            return Err(anyhow!(
                "resources mounts contain duplicate asset_dir: {}",
                mount.asset_dir
            ));
        }
    }
    Ok(())
}

pub(in crate::commands::build) fn toml_to_json_normalized(
    value: &TomlValue,
) -> anyhow::Result<JsonValue> {
    Ok(match value {
        TomlValue::String(v) => JsonValue::String(v.clone()),
        TomlValue::Integer(v) => JsonValue::Number((*v).into()),
        TomlValue::Float(v) => {
            let number = serde_json::Number::from_f64(*v)
                .ok_or_else(|| anyhow!("floating-point value is not representable as JSON"))?;
            JsonValue::Number(number)
        }
        TomlValue::Boolean(v) => JsonValue::Bool(*v),
        TomlValue::Datetime(v) => JsonValue::String(v.to_string()),
        TomlValue::Array(values) => JsonValue::Array(
            values
                .iter()
                .map(toml_to_json_normalized)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        TomlValue::Table(table) => {
            let mut keys = table.keys().cloned().collect::<Vec<_>>();
            keys.sort();

            let mut object = serde_json::Map::new();
            for key in keys {
                let nested = table
                    .get(&key)
                    .ok_or_else(|| anyhow!("internal error: missing table key"))?;
                object.insert(key, toml_to_json_normalized(nested)?);
            }
            JsonValue::Object(object)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::Path,
    };

    use serde_json::{Value as JsonValue, json};
    use sha2::Digest;
    use toml::Value as TomlValue;

    use crate::{
        commands::{
            build::{
                ManifestCapabilityPolicy, ManifestDependencyKind, ProjectDependency,
                ProjectDependencyComponent, ProjectDependencySource,
            },
            dependency_cache::{self, DependencyCacheEntry},
            plugin_sources,
        },
        lockfile::LockSourceKind,
    };

    use super::{
        AssetSource, EmbeddedResourceProviderCandidate, IMAGO_RESOURCES_CUSTOM_SECTION_NAME,
        LoadedResourceProvider, ManifestAsset, ResourceMergePolicy, cidr_network_ip,
        load_declared_resource_providers, load_embedded_resource_providers,
        load_resource_profile_expectations, load_resource_profile_expectations_from_providers,
        merge_resource_providers, normalize_wasi_http_outbound_rule, parse_resource_mount_entries,
        parse_resources_http_outbound, parse_resources_section, validate_resource_mount_uniqueness,
    };

    fn parse_table(raw: &str) -> toml::Table {
        toml::from_str::<TomlValue>(raw)
            .expect("toml should parse")
            .as_table()
            .expect("value should be table")
            .clone()
    }

    fn new_temp_dir(test_name: &str) -> std::path::PathBuf {
        let unique = format!(
            "imago-resource-tests-{}-{}-{}",
            test_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn push_u32_leb(bytes: &mut Vec<u8>, mut value: usize) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn wasm_module_with_custom_section(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut section = Vec::new();
        push_u32_leb(&mut section, name.len());
        section.extend_from_slice(name.as_bytes());
        section.extend_from_slice(payload);

        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        bytes.push(0);
        push_u32_leb(&mut bytes, section.len());
        bytes.extend_from_slice(&section);
        bytes
    }

    fn parse_service_resources(
        raw: &str,
        root: &Path,
    ) -> Option<crate::commands::build::ManifestResourcesConfig> {
        parse_resources_section(&parse_table(raw), &[], root).expect("resources should parse")
    }

    fn sample_embedded_provider(resources: JsonValue) -> LoadedResourceProvider {
        LoadedResourceProvider {
            expectation: crate::lockfile::ResourceProfileExpectation {
                resource: "resources".to_string(),
                profile_kind: "component-custom-section".to_string(),
                source_kind: LockSourceKind::Path,
                source: "registry/example-provider.wasm".to_string(),
                provider_dependency: Some("yieldspace:example-provider".to_string()),
                component_sha256: Some("a".repeat(64)),
                digest: "digest".to_string(),
            },
            provider_name: "yieldspace:example-provider".to_string(),
            resources,
            merge_policies: BTreeMap::new(),
        }
    }

    fn sample_gpio_provider() -> LoadedResourceProvider {
        let mut provider = sample_embedded_provider(json!({
            "gpio": {
                "digital_pins": [{
                    "label": "A27",
                    "aliases": ["blue-led", "gpio507"],
                    "value_path": "/sys/class/gpio/gpio507/value",
                    "supports_input": false,
                    "supports_output": true,
                    "default_active_level": "active-high",
                    "allow_pull_resistor": false
                }]
            }
        }));
        provider.merge_policies.insert(
            "gpio.digital_pins".to_string(),
            ResourceMergePolicy::Mergeable,
        );
        provider
    }

    #[test]
    fn normalize_wasi_http_outbound_rule_normalizes_cidr_network() {
        let normalized = normalize_wasi_http_outbound_rule("192.168.10.11/24", "field")
            .expect("cidr should normalize");
        assert_eq!(normalized, "192.168.10.0/24");
    }

    #[test]
    fn normalize_wasi_http_outbound_rule_rejects_wildcard() {
        let err = normalize_wasi_http_outbound_rule("*.example.com", "field")
            .expect_err("wildcard should be rejected");
        assert!(err.to_string().contains("wildcard is not supported"));
    }

    #[test]
    fn parse_resources_http_outbound_deduplicates_entries() {
        let table = parse_table(
            r#"
rules = ["example.com:443", " example.com:443 ", "192.168.0.1/24"]
"#,
        );
        let rules = parse_resources_http_outbound(table.get("rules"))
            .expect("http_outbound rules should parse");
        assert_eq!(rules, vec!["example.com:443", "192.168.0.0/24"]);
    }

    #[test]
    fn parse_resource_mount_entries_rejects_unknown_asset_dir() {
        let mounts = parse_table(
            r#"
entries = [{ asset_dir = "assets", guest_path = "/data" }]
"#,
        );
        let allowed = BTreeSet::from([std::path::PathBuf::from("public")]);
        let err = parse_resource_mount_entries(mounts.get("entries"), "resources.mounts", &allowed)
            .expect_err("unknown asset_dir should be rejected");
        assert!(
            err.to_string()
                .contains("must match a directory derived from assets")
        );
    }

    #[test]
    fn validate_resource_mount_uniqueness_rejects_duplicate_guest_path() {
        let mounts = vec![crate::commands::build::ManifestWasiMount {
            asset_dir: "assets".to_string(),
            guest_path: "/data".to_string(),
        }];
        let read_only_mounts = vec![crate::commands::build::ManifestWasiMount {
            asset_dir: "assets-ro".to_string(),
            guest_path: "/data".to_string(),
        }];

        let err = validate_resource_mount_uniqueness(&mounts, &read_only_mounts)
            .expect_err("duplicate guest path should fail");
        assert!(err.to_string().contains("duplicate guest_path"));
    }

    #[test]
    fn collect_allowed_resource_asset_dirs_from_assets_can_be_used_by_mount_parser() {
        let assets = vec![AssetSource {
            manifest_asset: ManifestAsset {
                path: "public/static/logo.svg".to_string(),
                extra: BTreeMap::new(),
            },
            source_path: std::path::PathBuf::from("public/static/logo.svg"),
        }];
        let allowed = super::collect_allowed_resource_asset_dirs(&assets);
        let mounts = parse_table(
            r#"
entries = [{ asset_dir = "public/static", guest_path = "/assets" }]
"#,
        );

        let parsed =
            parse_resource_mount_entries(mounts.get("entries"), "resources.mounts", &allowed)
                .expect("asset dir derived from assets should be accepted");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].guest_path, "/assets");
    }

    #[test]
    fn cidr_network_ip_masks_ipv4_bits() {
        let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 1, 2, 200));
        let masked = cidr_network_ip(ip, 16);
        assert_eq!(masked.to_string(), "10.1.0.0");
    }

    #[test]
    fn parse_resources_section_keeps_gpio_patch_when_profile_is_also_declared() {
        let root = new_temp_dir("gpio-profile-expand");
        write(
            &root.join("boards/milkv-duo-s/profile.toml"),
            br#"
[[digital_pins]]
soc_name = "A27"
aliases = ["blue-led"]
gpio_num = 507
value_path = "/sys/class/gpio/gpio507/value"
supports_input = false
supports_output = true
default_active_level = "active-high"
allow_pull_resistor = false
header = "onboard"
physical_pin = 0
"#,
        );
        let config = parse_table(
            r#"
[resources.gpio]
digital_pins = [{ label = "A27", aliases = ["blue-led", "status-led"] }]

[resources.gpio.profile]
path = "boards/milkv-duo-s/profile.toml"
"#,
        );
        let resources = parse_resources_section(&config, &[], &root)
            .expect("resources should parse")
            .expect("resources should exist");
        let gpio = resources
            .extra
            .get("gpio")
            .and_then(JsonValue::as_object)
            .expect("gpio should be an object");
        let digital_pins = gpio
            .get("digital_pins")
            .and_then(JsonValue::as_array)
            .expect("digital_pins patch should be preserved");
        assert_eq!(digital_pins.len(), 1);
        assert_eq!(
            digital_pins[0]["label"],
            JsonValue::String("A27".to_string())
        );
        assert_eq!(
            digital_pins[0]["aliases"],
            JsonValue::Array(vec![
                JsonValue::String("blue-led".to_string()),
                JsonValue::String("status-led".to_string()),
            ])
        );
        assert!(gpio.get("profile").is_none());

        let expectations =
            load_resource_profile_expectations(&config, &root).expect("expectations should parse");
        assert_eq!(expectations.len(), 1);
        assert_eq!(expectations[0].resource, "gpio");
        assert_eq!(expectations[0].profile_kind, "path");
        assert_eq!(
            expectations[0].source,
            "boards/milkv-duo-s/profile.toml".to_string()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn merge_resource_providers_emits_provider_resources_without_service_resources() {
        let merged = merge_resource_providers(None, BTreeMap::new(), &[sample_gpio_provider()])
            .expect("provider resources should merge");
        let resources = merged.expect("provider resources should exist");
        let digital_pins = resources
            .extra
            .get("gpio")
            .and_then(JsonValue::as_object)
            .and_then(|gpio| gpio.get("digital_pins"))
            .and_then(JsonValue::as_array)
            .expect("digital_pins should be synthesized from provider");
        assert_eq!(digital_pins.len(), 1);
        assert_eq!(
            digital_pins[0]["label"],
            JsonValue::String("A27".to_string())
        );
    }

    #[test]
    fn merge_resource_providers_applies_gpio_keyed_patch_for_known_label() {
        let service_resources = parse_service_resources(
            r#"
[resources.gpio]
digital_pins = [{ label = "A27", aliases = ["blue-led", "status-led", "gpio507"] }]
"#,
            Path::new("."),
        );

        let merged = merge_resource_providers(
            service_resources,
            BTreeMap::new(),
            &[sample_gpio_provider()],
        )
        .expect("service patch should merge");
        let resources = merged.expect("merged resources should exist");
        let digital_pins = resources
            .extra
            .get("gpio")
            .and_then(JsonValue::as_object)
            .and_then(|gpio| gpio.get("digital_pins"))
            .and_then(JsonValue::as_array)
            .expect("digital_pins should exist");
        assert_eq!(
            digital_pins[0]["aliases"],
            JsonValue::Array(vec![
                JsonValue::String("blue-led".to_string()),
                JsonValue::String("status-led".to_string()),
                JsonValue::String("gpio507".to_string()),
            ])
        );
        assert_eq!(
            load_resource_profile_expectations_from_providers(&[sample_gpio_provider()])[0]
                .profile_kind,
            "component-custom-section"
        );
    }

    #[test]
    fn merge_resource_providers_rejects_unknown_gpio_patch_label() {
        let service_resources = parse_service_resources(
            r#"
[resources.gpio]
digital_pins = [{ label = "B12", aliases = ["j3:13"] }]
"#,
            Path::new("."),
        );

        let err = merge_resource_providers(
            service_resources,
            BTreeMap::new(),
            &[sample_gpio_provider()],
        )
        .expect_err("unknown label patch must fail");
        assert!(
            err.to_string()
                .contains("patch references unknown label: B12")
        );
    }

    #[test]
    fn merge_resource_providers_rejects_sealed_override() {
        let service_resources = parse_service_resources(
            r#"
[resources.policy]
mode = "loose"
"#,
            Path::new("."),
        );
        let provider = sample_embedded_provider(json!({
            "policy": { "mode": "strict" }
        }));

        let err = merge_resource_providers(service_resources, BTreeMap::new(), &[provider])
            .expect_err("sealed provider path must reject overrides");
        assert!(
            err.to_string()
                .contains("must not override sealed provider path")
        );
    }

    #[test]
    fn merge_resource_providers_requires_required_override() {
        let mut provider = sample_embedded_provider(json!({
            "policy": { "mode": "strict" }
        }));
        provider
            .merge_policies
            .insert("policy".to_string(), ResourceMergePolicy::Required);

        let err = merge_resource_providers(None, BTreeMap::new(), &[provider.clone()])
            .expect_err("required provider path must demand a service override");
        assert!(
            err.to_string()
                .contains("must override required provider path resources.policy")
        );

        let service_resources = parse_service_resources(
            r#"
[resources.policy]
mode = "loose"
"#,
            Path::new("."),
        );
        let merged = merge_resource_providers(service_resources, BTreeMap::new(), &[provider])
            .expect("required provider override should succeed");
        assert_eq!(
            merged
                .expect("merged resources should exist")
                .extra
                .get("policy"),
            Some(&json!({ "mode": "loose" }))
        );
    }

    #[test]
    fn merge_resource_providers_requires_actual_required_override() {
        let mut provider = sample_embedded_provider(json!({
            "policy": { "mode": "strict" }
        }));
        provider
            .merge_policies
            .insert("policy".to_string(), ResourceMergePolicy::Required);

        let service_resources = parse_service_resources(
            r#"
[resources.policy]
detail = "note"
"#,
            Path::new("."),
        );

        let err = merge_resource_providers(service_resources, BTreeMap::new(), &[provider])
            .expect_err("new child keys alone must not satisfy required overrides");
        assert!(
            err.to_string()
                .contains("must override required provider path resources.policy"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn merge_resource_providers_requires_gpio_field_override_not_label_only_patch() {
        let mut provider = sample_gpio_provider();
        provider.merge_policies.insert(
            "gpio.digital_pins".to_string(),
            ResourceMergePolicy::Required,
        );
        let service_resources = parse_service_resources(
            r#"
[resources.gpio]
digital_pins = [{ label = "A27" }]
"#,
            Path::new("."),
        );

        let err = merge_resource_providers(service_resources, BTreeMap::new(), &[provider])
            .expect_err("label-only patch must not satisfy required gpio overrides");
        assert!(
            err.to_string()
                .contains("must override required provider path resources.gpio.digital_pins"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn merge_resource_providers_rejects_duplicate_gpio_patch_labels() {
        let service_resources = parse_service_resources(
            r#"
[resources.gpio]
digital_pins = [
  { label = "A27", aliases = ["blue-led"] },
  { label = "A27", aliases = ["status-led"] }
]
"#,
            Path::new("."),
        );

        let err = merge_resource_providers(
            service_resources,
            BTreeMap::new(),
            &[sample_gpio_provider()],
        )
        .expect_err("duplicate gpio patch labels must be rejected");
        assert!(
            err.to_string()
                .contains("resources.gpio.digital_pins contains duplicate patch label: A27"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn merge_resource_providers_rejects_provider_gpio_alias_conflicts() {
        let mut first = sample_gpio_provider();
        first.resources = json!({
            "gpio": {
                "digital_pins": [{
                    "label": "A27",
                    "aliases": ["blue-led"],
                    "value_path": "/sys/class/gpio/gpio507/value",
                    "supports_input": false,
                    "supports_output": true,
                    "default_active_level": "active-high",
                    "allow_pull_resistor": false
                }]
            }
        });
        let mut second = sample_gpio_provider();
        second.provider_name = "yieldspace:other-board".to_string();
        second.resources = json!({
            "gpio": {
                "digital_pins": [{
                    "label": "status-led",
                    "aliases": ["blue-led"],
                    "value_path": "/sys/class/gpio/gpio508/value",
                    "supports_input": false,
                    "supports_output": true,
                    "default_active_level": "active-high",
                    "allow_pull_resistor": false
                }]
            }
        });

        let err = merge_resource_providers(None, BTreeMap::new(), &[first, second])
            .expect_err("provider alias collisions must be rejected");
        assert!(
            err.to_string()
                .contains("resource provider 'yieldspace:other-board' conflicts"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string()
                .contains("resources.gpio.digital_pins[blue-led]"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn merge_resource_providers_rejects_path_provider_conflict_with_embedded_provider() {
        let root = new_temp_dir("gpio-profile-conflict");
        write(
            &root.join("boards/milkv-duo-s/profile.toml"),
            br#"
[[digital_pins]]
soc_name = "A27"
gpio_num = 507
value_path = "/sys/class/gpio/gpio507/value"
supports_input = false
supports_output = true
default_active_level = "active-high"
allow_pull_resistor = false
header = "onboard"
physical_pin = 0
"#,
        );
        let config = parse_table(
            r#"
[resources.gpio]
digital_pins = []

[resources.gpio.profile]
path = "boards/milkv-duo-s/profile.toml"
"#,
        );
        let mut providers =
            load_declared_resource_providers(&config, &root).expect("profile provider should load");
        providers.push(sample_gpio_provider());

        let err = merge_resource_providers(None, BTreeMap::new(), &providers)
            .expect_err("path provider and embedded provider should conflict");
        assert!(err.to_string().contains("conflicts with"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_embedded_resource_providers_extracts_custom_section_from_cached_component() {
        let root = new_temp_dir("embedded-provider");
        let component_payload = json!({
            "schema_version": 1,
            "resources": {
                "gpio": {
                    "digital_pins": [{
                        "label": "A27",
                        "aliases": ["blue-led"],
                        "value_path": "/sys/class/gpio/gpio507/value",
                        "supports_input": false,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": false
                    }]
                }
            },
            "merge_policies": {
                "gpio.digital_pins": "mergeable"
            }
        });
        let section_bytes =
            serde_json::to_vec(&component_payload).expect("custom section payload should encode");
        let component_bytes =
            wasm_module_with_custom_section(IMAGO_RESOURCES_CUSTOM_SECTION_NAME, &section_bytes);
        let component_sha = hex::encode(sha2::Sha256::digest(&component_bytes));

        let dependency = ProjectDependency {
            name: "yieldspace:milkv-duo-s-gpio".to_string(),
            version: "0.1.0".to_string(),
            kind: ManifestDependencyKind::Wasm,
            wit: ProjectDependencySource {
                source_kind: plugin_sources::SourceKind::Path,
                source: "registry/yieldspace-milkv-duo-s-gpio".to_string(),
                registry: None,
                sha256: None,
            },
            requires: vec![],
            component: Some(ProjectDependencyComponent {
                source_kind: plugin_sources::SourceKind::Path,
                source: "registry/yieldspace-milkv-duo-s-gpio-provider.wasm".to_string(),
                registry: None,
                sha256: Some(component_sha.clone()),
            }),
            capabilities: ManifestCapabilityPolicy::default(),
        };
        dependency_cache::save_entry(
            &root,
            &DependencyCacheEntry {
                name: dependency.name.clone(),
                resolved_package_name: None,
                version: dependency.version.clone(),
                kind: "wasm".to_string(),
                wit_source: dependency.wit.source.clone(),
                wit_registry: dependency.wit.registry.clone(),
                wit_sha256: dependency.wit.sha256.clone(),
                wit_path: "wit/deps/yieldspace-milkv-duo-s-gpio-0.1.0".to_string(),
                wit_digest: "sha256-placeholder".to_string(),
                wit_source_fingerprint: None,
                component_source: dependency
                    .component
                    .as_ref()
                    .map(|value| value.source.clone()),
                component_registry: dependency
                    .component
                    .as_ref()
                    .and_then(|value| value.registry.clone()),
                component_sha256: Some(component_sha.clone()),
                component_source_fingerprint: None,
                component_world_foreign_packages: vec![],
                component_world_foreign_packages_recorded: true,
                transitive_packages: vec![],
            },
        )
        .expect("dependency cache entry should be written");
        write(
            &dependency_cache::cache_component_path(&root, &dependency.name, &component_sha),
            &component_bytes,
        );

        let providers = load_embedded_resource_providers(
            &root,
            &[EmbeddedResourceProviderCandidate {
                project_dependency: &dependency,
                resolved_dependency_name: &dependency.name,
                component_sha256: &component_sha,
            }],
        )
        .expect("embedded provider should load");

        assert_eq!(providers.len(), 1);
        let provider = &providers[0];
        assert_eq!(provider.provider_name, "yieldspace:milkv-duo-s-gpio");
        assert_eq!(
            provider.expectation.profile_kind,
            "component-custom-section".to_string()
        );
        assert_eq!(
            provider.expectation.provider_dependency.as_deref(),
            Some("yieldspace:milkv-duo-s-gpio")
        );
        assert_eq!(
            provider.expectation.component_sha256.as_deref(),
            Some(component_sha.as_str())
        );
        assert_eq!(
            provider.resources["gpio"]["digital_pins"][0]["label"],
            JsonValue::String("A27".to_string())
        );
        assert_eq!(
            provider.merge_policies.get("gpio.digital_pins"),
            Some(&ResourceMergePolicy::Mergeable)
        );

        let _ = fs::remove_dir_all(root);
    }
}
