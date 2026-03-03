//! JSON Schema generation for `imago.toml` and `imagod.toml`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use imago_project_config::{IMAGO_SCHEMA_FILENAME, ImagoTomlDocument};
use imagod_config::{IMAGOD_SCHEMA_FILENAME, ImagodTomlDocument};
use schemars::{JsonSchema, schema_for};
use serde_json::Value as JsonValue;

const JSON_SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

pub fn generate_all(workspace_root: &Path) -> Result<()> {
    let schema_dir = workspace_root.join("schemas");
    fs::create_dir_all(&schema_dir)
        .with_context(|| format!("failed to create schema dir {}", schema_dir.display()))?;

    let imago_schema = schema_value_for::<ImagoTomlDocument>()?;
    let imagod_schema = schema_value_for::<ImagodTomlDocument>()?;

    write_schema_if_changed(&schema_dir.join(IMAGO_SCHEMA_FILENAME), &imago_schema)?;
    write_schema_if_changed(&schema_dir.join(IMAGOD_SCHEMA_FILENAME), &imagod_schema)?;

    Ok(())
}

fn schema_value_for<T>() -> Result<JsonValue>
where
    T: JsonSchema,
{
    let schema = schema_for!(T);
    let mut value = serde_json::to_value(schema).context("failed to serialize generated schema")?;
    let object = value
        .as_object_mut()
        .context("generated schema root must be a JSON object")?;
    object.insert(
        "$schema".to_string(),
        JsonValue::String(JSON_SCHEMA_DRAFT_2020_12.to_string()),
    );
    Ok(value)
}

fn write_schema_if_changed(path: &Path, schema: &JsonValue) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(schema)
        .with_context(|| format!("failed to encode schema {}", path.display()))?;
    bytes.push(b'\n');

    if let Ok(existing) = fs::read(path)
        && existing == bytes
    {
        return Ok(());
    }

    fs::write(path, bytes).with_context(|| format!("failed to write schema {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value as JsonValue;

    use super::generate_all;

    #[test]
    fn generate_all_is_deterministic_and_writes_both_schema_files() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path();

        generate_all(root).expect("first generation should succeed");
        let imago_path = root.join("schemas/imago.schema.json");
        let imagod_path = root.join("schemas/imagod.schema.json");
        assert!(imago_path.exists(), "imago schema should be generated");
        assert!(imagod_path.exists(), "imagod schema should be generated");

        let first_imago = fs::read_to_string(&imago_path).expect("imago schema should be readable");
        let first_imagod =
            fs::read_to_string(&imagod_path).expect("imagod schema should be readable");

        generate_all(root).expect("second generation should also succeed");

        let second_imago =
            fs::read_to_string(&imago_path).expect("imago schema should still be readable");
        let second_imagod =
            fs::read_to_string(&imagod_path).expect("imagod schema should still be readable");

        assert_eq!(
            first_imago, second_imago,
            "imago schema should be deterministic"
        );
        assert_eq!(
            first_imagod, second_imagod,
            "imagod schema should be deterministic"
        );
        assert!(
            first_imago.contains("\"$schema\""),
            "imago schema should contain draft declaration"
        );
        assert!(
            first_imagod.contains("\"$schema\""),
            "imagod schema should contain draft declaration"
        );

        let imago_json: JsonValue =
            serde_json::from_str(&first_imago).expect("imago schema should parse as json");
        let props = imago_json
            .get("properties")
            .and_then(JsonValue::as_object)
            .expect("imago root properties should be object");
        let defs = imago_json
            .get("$defs")
            .and_then(JsonValue::as_object)
            .expect("imago $defs should be object");
        let target_props = defs
            .get("TargetEntry")
            .and_then(|entry| entry.get("properties"))
            .and_then(JsonValue::as_object)
            .expect("TargetEntry.properties should be object");
        let target_required = defs
            .get("TargetEntry")
            .and_then(|entry| entry.get("required"))
            .and_then(JsonValue::as_array)
            .expect("TargetEntry.required should be array");
        let dependency_props = defs
            .get("DependencyEntry")
            .and_then(|entry| entry.get("properties"))
            .and_then(JsonValue::as_object)
            .expect("DependencyEntry.properties should be object");
        let dependency_required = defs
            .get("DependencyEntry")
            .and_then(|entry| entry.get("required"))
            .and_then(JsonValue::as_array)
            .expect("DependencyEntry.required should be array");
        let dependency_component_props = defs
            .get("DependencyComponentEntry")
            .and_then(|entry| entry.get("properties"))
            .and_then(JsonValue::as_object)
            .expect("DependencyComponentEntry.properties should be object");
        let binding_props = defs
            .get("BindingEntry")
            .and_then(|entry| entry.get("properties"))
            .and_then(JsonValue::as_object)
            .expect("BindingEntry.properties should be object");
        let binding_required = defs
            .get("BindingEntry")
            .and_then(|entry| entry.get("required"))
            .and_then(JsonValue::as_array)
            .expect("BindingEntry.required should be array");

        assert!(
            !props.contains_key("capabilirties"),
            "capabilirties must not be exposed in schema properties"
        );
        assert!(
            !props.contains_key("runtime"),
            "legacy runtime table must not be exposed in schema properties"
        );
        assert!(
            !target_props.contains_key("ca_cert"),
            "legacy target.ca_cert must not be exposed in schema properties"
        );
        assert!(
            !target_props.contains_key("client_cert"),
            "legacy target.client_cert must not be exposed in schema properties"
        );
        assert!(
            !target_props.contains_key("known_hosts"),
            "legacy target.known_hosts must not be exposed in schema properties"
        );
        assert!(
            target_required
                .iter()
                .any(|value| value.as_str() == Some("remote")),
            "target.remote must be required in schema"
        );
        assert!(
            !dependency_props.contains_key("name"),
            "legacy dependencies[].name must not be exposed in schema properties"
        );
        assert!(
            dependency_required
                .iter()
                .any(|value| value.as_str() == Some("version")),
            "dependencies[].version must be required in schema"
        );
        assert!(
            dependency_required
                .iter()
                .any(|value| value.as_str() == Some("kind")),
            "dependencies[].kind must be required in schema"
        );
        assert!(
            !dependency_component_props.contains_key("source"),
            "legacy dependencies[].component.source must not be exposed in schema properties"
        );
        assert!(
            !binding_props.contains_key("target"),
            "legacy bindings[].target must not be exposed in schema properties"
        );
        assert!(
            binding_required
                .iter()
                .any(|value| value.as_str() == Some("name")),
            "bindings[].name must be required in schema"
        );
        assert!(
            binding_required
                .iter()
                .any(|value| value.as_str() == Some("version")),
            "bindings[].version must be required in schema"
        );
        assert!(
            !defs.contains_key("LegacyRuntimeSection"),
            "legacy runtime section type must not be exposed in schema definitions"
        );
    }
}
