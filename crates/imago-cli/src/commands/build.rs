use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use toml::Value as TomlValue;

use crate::{cli::BuildArgs, commands::CommandResult};

const DEFAULT_TARGET_NAME: &str = "default";

#[derive(Debug, Clone)]
pub struct TargetConfig {
    pub remote: String,
    pub server_name: Option<String>,
    pub ca_cert: Option<PathBuf>,
    pub client_cert: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DeployTargetConfig {
    pub remote: String,
    pub server_name: Option<String>,
    pub ca_cert: PathBuf,
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
}

impl TargetConfig {
    pub fn as_manifest_target_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("remote".to_string(), self.remote.clone());
        if let Some(server_name) = &self.server_name {
            map.insert("server_name".to_string(), server_name.clone());
        }
        map
    }

    pub fn require_deploy_credentials(&self) -> anyhow::Result<DeployTargetConfig> {
        let ca_cert = self
            .ca_cert
            .clone()
            .ok_or_else(|| anyhow!("target is missing required key: ca_cert"))?;
        let client_cert = self
            .client_cert
            .clone()
            .ok_or_else(|| anyhow!("target is missing required key: client_cert"))?;
        let client_key = self
            .client_key
            .clone()
            .ok_or_else(|| anyhow!("target is missing required key: client_key"))?;

        Ok(DeployTargetConfig {
            remote: self.remote.clone(),
            server_name: self.server_name.clone(),
            ca_cert,
            client_cert,
            client_key,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BuildOutput {
    pub manifest_path: PathBuf,
    pub manifest_bytes: Vec<u8>,
    pub target: TargetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    name: String,
    main: String,
    #[serde(rename = "type")]
    app_type: String,
    target: BTreeMap<String, String>,
    vars: BTreeMap<String, String>,
    secrets: BTreeMap<String, String>,
    assets: Vec<ManifestAsset>,
    #[serde(default)]
    bindings: Vec<ManifestBinding>,
    dependencies: Vec<JsonValue>,
    hash: ManifestHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestAsset {
    path: String,
    #[serde(flatten)]
    extra: BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestBinding {
    target: String,
    wit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestHash {
    algorithm: String,
    value: String,
    targets: Vec<String>,
}

#[derive(Debug, Clone)]
struct AssetSource {
    manifest_asset: ManifestAsset,
    source_path: PathBuf,
}

#[derive(Debug, Clone)]
enum BuildCommand {
    Shell(String),
    Argv(Vec<String>),
}

pub fn run(args: BuildArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(args: BuildArgs, project_root: &Path) -> CommandResult {
    match run_inner(args, project_root) {
        Ok(()) => CommandResult {
            exit_code: 0,
            stderr: None,
        },
        Err(err) => CommandResult {
            exit_code: 2,
            stderr: Some(err.to_string()),
        },
    }
}

fn run_inner(args: BuildArgs, project_root: &Path) -> anyhow::Result<()> {
    build_project(args.env.as_deref(), &args.target, project_root)?;
    Ok(())
}

pub fn build_project(
    env: Option<&str>,
    target_name: &str,
    project_root: &Path,
) -> anyhow::Result<BuildOutput> {
    if let Some(env_name) = env {
        validate_env_name(env_name)?;
    }
    let root = load_resolved_toml(project_root, env)?;
    let secrets = load_env_file(project_root, env)?;

    let command = parse_build_command(&root)?;

    let name = required_string(&root, "name")?;
    validate_service_name(&name)?;

    let main_raw = required_string(&root, "main")?;
    let source_main_path = normalize_relative_path(&main_raw, "main")?;

    let app_type = required_string(&root, "type")?;
    validate_app_type(&app_type)?;

    let vars = parse_string_table(root.get("vars"), "vars")?;
    let bindings = parse_bindings(root.get("bindings"))?;
    let dependencies = parse_dependencies(root.get("dependencies"))?;
    let target = parse_target(&root, target_name, project_root)?;

    run_build_command(command.as_ref(), project_root, &secrets)?;

    ensure_file_exists(project_root, &source_main_path, "main")?;
    let materialized_main_path = materialize_hashed_wasm(project_root, &source_main_path, &name)?;
    let manifest_main = materialized_main_path
        .file_name()
        .ok_or_else(|| anyhow!("materialized wasm filename is missing"))?
        .to_os_string();

    let assets = parse_assets(root.get("assets"), project_root)?;

    let mut manifest = Manifest {
        name,
        main: normalized_path_to_string(Path::new(&manifest_main)),
        app_type,
        target: target.as_manifest_target_map(),
        vars,
        secrets,
        assets: assets
            .iter()
            .map(|entry| entry.manifest_asset.clone())
            .collect(),
        bindings,
        dependencies,
        hash: ManifestHash {
            algorithm: "sha256".to_string(),
            value: String::new(),
            targets: vec![
                "wasm".to_string(),
                "manifest".to_string(),
                "assets".to_string(),
            ],
        },
    };

    manifest.hash.value =
        compute_manifest_hash(project_root, &materialized_main_path, &assets, &manifest)?;

    let mut manifest_bytes =
        serde_json::to_vec_pretty(&manifest).context("failed to serialize build manifest")?;
    manifest_bytes.push(b'\n');

    let manifest_path = resolve_manifest_output_path(env)?;
    let output_path = project_root.join(&manifest_path);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create manifest directory: {}", parent.display())
        })?;
    }
    fs::write(&output_path, &manifest_bytes)
        .with_context(|| format!("failed to write manifest: {}", output_path.display()))?;

    Ok(BuildOutput {
        manifest_path,
        manifest_bytes,
        target,
    })
}

pub(crate) fn load_resolved_toml(
    project_root: &Path,
    env: Option<&str>,
) -> anyhow::Result<toml::Table> {
    let path = project_root.join("imago.toml");
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: TomlValue = toml::from_str(&raw).context("failed to parse imago.toml")?;
    let mut root = parsed
        .as_table()
        .cloned()
        .ok_or_else(|| anyhow!("imago.toml root must be a table"))?;

    if let Some(env_name) = env {
        let envs = root
            .get("env")
            .and_then(TomlValue::as_table)
            .ok_or_else(|| anyhow!("env '{}' is not defined in imago.toml", env_name))?;
        let env_value = envs
            .get(env_name)
            .ok_or_else(|| anyhow!("env '{}' is not defined in imago.toml", env_name))?;
        let env_table = env_value
            .as_table()
            .ok_or_else(|| anyhow!("env '{}' must be a table", env_name))?;
        let replacements = env_table
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();

        for (key, value) in replacements {
            root.insert(key, value);
        }
    }

    Ok(root)
}

fn parse_build_command(root: &toml::Table) -> anyhow::Result<Option<BuildCommand>> {
    let Some(build_section) = root.get("build") else {
        return Ok(None);
    };
    let build_table = build_section
        .as_table()
        .ok_or_else(|| anyhow!("build must be a table"))?;
    let Some(command_value) = build_table.get("command") else {
        return Ok(None);
    };

    if let Some(shell) = command_value.as_str() {
        if shell.trim().is_empty() {
            return Err(anyhow!("build.command must not be empty"));
        }
        return Ok(Some(BuildCommand::Shell(shell.to_string())));
    }

    let Some(values) = command_value.as_array() else {
        return Err(anyhow!("build.command must be string or array of strings"));
    };
    if values.is_empty() {
        return Err(anyhow!("build.command array must not be empty"));
    }

    let mut argv = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let part = value.as_str().ok_or_else(|| {
            anyhow!("build.command array must contain only strings (invalid index {index})")
        })?;
        argv.push(part.to_string());
    }

    Ok(Some(BuildCommand::Argv(argv)))
}

fn run_build_command(
    command: Option<&BuildCommand>,
    project_root: &Path,
    env_vars: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let Some(command) = command else {
        return Ok(());
    };

    let mut process = match command {
        BuildCommand::Shell(script) => {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(script);
            cmd
        }
        BuildCommand::Argv(argv) => {
            let mut cmd = Command::new(&argv[0]);
            for arg in argv.iter().skip(1) {
                cmd.arg(arg);
            }
            cmd
        }
    };

    process.current_dir(project_root);
    for (key, value) in env_vars {
        process.env(key, value);
    }

    let status = process.status().context("failed to run build.command")?;

    if status.success() {
        return Ok(());
    }

    if let Some(code) = status.code() {
        Err(anyhow!("build.command failed with exit code {code}"))
    } else {
        Err(anyhow!("build.command was terminated by signal"))
    }
}

fn load_env_file(
    project_root: &Path,
    env: Option<&str>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let Some(env_name) = env else {
        return Ok(BTreeMap::new());
    };
    validate_env_name(env_name)?;

    let path = project_root.join(format!(".env.{env_name}"));
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read env file: {}", path.display()))?;

    let mut values = BTreeMap::new();
    for (line_no, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let body = if let Some(rest) = trimmed.strip_prefix("export ") {
            rest.trim_start()
        } else {
            trimmed
        };

        let (raw_key, raw_value) = body.split_once('=').ok_or_else(|| {
            anyhow!(
                "invalid env line at {}:{} (expected KEY=VALUE)",
                path.display(),
                line_no + 1
            )
        })?;

        let key = raw_key.trim();
        if key.is_empty() {
            return Err(anyhow!(
                "invalid env line at {}:{} (empty key)",
                path.display(),
                line_no + 1
            ));
        }

        let mut value = raw_value.trim().to_string();
        if value.len() >= 2 {
            let bytes = value.as_bytes();
            if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
                || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            {
                value = value[1..value.len() - 1].to_string();
            }
        }

        values.insert(key.to_string(), value);
    }

    Ok(values)
}

fn required_string(root: &toml::Table, key: &str) -> anyhow::Result<String> {
    let value = root
        .get(key)
        .ok_or_else(|| anyhow!("imago.toml missing required key: {key}"))?;
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("imago.toml key '{}' must be a string", key))?
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(anyhow!("imago.toml key '{}' must not be empty", key));
    }
    Ok(text)
}

fn validate_service_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        return Err(anyhow!("name must not be empty"));
    }
    if name.len() > 63 {
        return Err(anyhow!("name must be 63 characters or fewer"));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(anyhow!("name contains invalid path characters: {}", name));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(anyhow!("name contains unsupported characters: {}", name));
    }
    Ok(())
}

fn validate_env_name(env_name: &str) -> anyhow::Result<()> {
    if env_name.is_empty() {
        return Err(anyhow!("env name must not be empty"));
    }
    if env_name.contains('/') || env_name.contains('\\') || env_name.contains("..") {
        return Err(anyhow!(
            "env name contains invalid path characters: {}",
            env_name
        ));
    }
    if !env_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(anyhow!(
            "env name contains unsupported characters: {}",
            env_name
        ));
    }
    Ok(())
}

fn validate_app_type(app_type: &str) -> anyhow::Result<()> {
    match app_type {
        "cli" | "http" | "socket" => Ok(()),
        _ => Err(anyhow!(
            "type must be one of: cli, http, socket (got: {})",
            app_type
        )),
    }
}

fn normalize_relative_path(raw: &str, field_name: &str) -> anyhow::Result<PathBuf> {
    if raw.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(anyhow!("{field_name} must be a relative path: {raw}"));
    }
    if raw.contains('\\') {
        return Err(anyhow!("{field_name} must not contain backslashes: {raw}"));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(anyhow!("{field_name} must not be windows-prefixed: {raw}"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir => {
                return Err(anyhow!(
                    "{field_name} must not contain path traversal: {raw}"
                ));
            }
            _ => {
                return Err(anyhow!(
                    "{field_name} contains invalid path component: {raw}"
                ));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} is invalid: {raw}"));
    }

    Ok(normalized)
}

fn normalized_path_to_string(path: &Path) -> String {
    path.iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn ensure_file_exists(
    project_root: &Path,
    relative: &Path,
    field_name: &str,
) -> anyhow::Result<()> {
    let path = project_root.join(relative);
    let metadata = fs::metadata(&path)
        .with_context(|| format!("{} file is not accessible: {}", field_name, path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!(
            "{} path is not a file: {}",
            field_name,
            path.display()
        ));
    }
    Ok(())
}

fn parse_string_table(
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

fn parse_dependencies(value: Option<&TomlValue>) -> anyhow::Result<Vec<JsonValue>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("dependencies must be an array"))?;
    let mut dependencies = Vec::with_capacity(array.len());
    for item in array {
        dependencies.push(toml_to_json_normalized(item)?);
    }
    Ok(dependencies)
}

fn parse_bindings(value: Option<&TomlValue>) -> anyhow::Result<Vec<ManifestBinding>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("bindings must be an array"))?;
    let mut bindings = Vec::with_capacity(array.len());
    for (index, entry) in array.iter().enumerate() {
        let table = entry
            .as_table()
            .ok_or_else(|| anyhow!("bindings[{index}] must be a table"))?;
        let target = table
            .get("target")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("bindings[{index}].target must be a string"))?
            .trim()
            .to_string();
        let wit = table
            .get("wit")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("bindings[{index}].wit must be a string"))?
            .trim()
            .to_string();

        if target.is_empty() {
            return Err(anyhow!("bindings[{index}].target must not be empty"));
        }
        if wit.is_empty() {
            return Err(anyhow!("bindings[{index}].wit must not be empty"));
        }
        validate_service_name(&target).map_err(|e| {
            anyhow!(
                "bindings[{index}].target is invalid: {}",
                e.to_string().replace("name ", "")
            )
        })?;

        bindings.push(ManifestBinding { target, wit });
    }
    Ok(bindings)
}

pub(crate) fn parse_target(
    root: &toml::Table,
    target_name: &str,
    project_root: &Path,
) -> anyhow::Result<TargetConfig> {
    let targets = root
        .get("target")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("imago.toml missing required key: target"))?;
    let raw_target = targets
        .get(target_name)
        .ok_or_else(|| anyhow!("target '{}' is not defined in imago.toml", target_name))?;
    let target_table = raw_target
        .as_table()
        .ok_or_else(|| anyhow!("target '{}' must be a table", target_name))?;

    let remote = target_table
        .get("remote")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("target '{}' is missing required key: remote", target_name))?
        .to_string();

    let server_name = optional_string(target_table, "server_name")?;
    let ca_cert = optional_target_cert_path(target_table, "ca_cert", project_root)?;
    let client_cert = optional_target_cert_path(target_table, "client_cert", project_root)?;
    let client_key = optional_target_cert_path(target_table, "client_key", project_root)?;

    Ok(TargetConfig {
        remote,
        server_name,
        ca_cert,
        client_cert,
        client_key,
    })
}

pub(crate) fn optional_string(table: &toml::Table, key: &str) -> anyhow::Result<Option<String>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("target key '{}' must be a string", key))?
        .to_string();
    Ok(Some(text))
}

pub(crate) fn optional_target_cert_path(
    table: &toml::Table,
    key: &str,
    project_root: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("target key '{}' must be a string", key))?;
    let normalized = normalize_target_cert_path(text, key)?;
    if normalized.is_absolute() {
        Ok(Some(normalized))
    } else {
        Ok(Some(project_root.join(normalized)))
    }
}

fn normalize_target_cert_path(raw: &str, key: &str) -> anyhow::Result<PathBuf> {
    if raw.is_empty() {
        return Err(anyhow!("target key '{}' must not be empty", key));
    }

    let path = Path::new(raw);
    if raw.contains('\\') {
        return Err(anyhow!(
            "target key '{}' must not contain backslashes: {}",
            key,
            raw
        ));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(anyhow!(
            "target key '{}' must not be windows-prefixed: {}",
            key,
            raw
        ));
    }

    let is_absolute = path.is_absolute();
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::RootDir => {}
            Component::ParentDir => {
                return Err(anyhow!(
                    "target key '{}' must not contain path traversal: {}",
                    key,
                    raw
                ));
            }
            _ => {
                return Err(anyhow!("target key '{}' is invalid: {}", key, raw));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("target key '{}' must not be empty", key));
    }

    if is_absolute {
        Ok(Path::new("/").join(normalized))
    } else {
        Ok(normalized)
    }
}

fn parse_assets(
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

fn toml_to_json_normalized(value: &TomlValue) -> anyhow::Result<JsonValue> {
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

fn compute_manifest_hash(
    project_root: &Path,
    main_path: &Path,
    assets: &[AssetSource],
    manifest: &Manifest,
) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_file_into(&mut hasher, &project_root.join(main_path), "main wasm")?;

    let normalized_manifest =
        serde_json::to_vec(manifest).context("failed to serialize normalized manifest for hash")?;
    hasher.update(&normalized_manifest);

    let mut sorted_assets = assets.iter().collect::<Vec<_>>();
    sorted_assets.sort_by(|a, b| a.manifest_asset.path.cmp(&b.manifest_asset.path));
    for asset in sorted_assets {
        hash_file_into(
            &mut hasher,
            &project_root.join(&asset.source_path),
            "asset for hash",
        )?;
    }

    Ok(hex::encode(hasher.finalize()))
}

fn compute_sha256_hex(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_file_into(&mut hasher, path, "file for sha256")?;
    Ok(hex::encode(hasher.finalize()))
}

fn hash_file_into(hasher: &mut Sha256, path: &Path, context_label: &str) -> anyhow::Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(())
}

fn ensure_build_dir(project_root: &Path) -> anyhow::Result<PathBuf> {
    let build_dir = project_root.join("build");
    fs::create_dir_all(&build_dir)
        .with_context(|| format!("failed to create build directory: {}", build_dir.display()))?;
    Ok(build_dir)
}

fn materialize_hashed_wasm(
    project_root: &Path,
    source_main_path: &Path,
    service_name: &str,
) -> anyhow::Result<PathBuf> {
    let source = project_root.join(source_main_path);
    let digest = compute_sha256_hex(&source)?;
    let build_dir = ensure_build_dir(project_root)?;
    let file_name = format!("{digest}-{service_name}.wasm");
    let destination = build_dir.join(file_name);

    if destination.exists() {
        let metadata = fs::metadata(&destination).with_context(|| {
            format!(
                "failed to inspect materialized wasm: {}",
                destination.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(anyhow!(
                "materialized wasm path is not a file: {}",
                destination.display()
            ));
        }

        let existing_digest = compute_sha256_hex(&destination).with_context(|| {
            format!(
                "failed to verify materialized wasm hash: {}",
                destination.display()
            )
        })?;
        if existing_digest != digest {
            copy_materialized_wasm(&source, &destination)?;
        }
    } else {
        copy_materialized_wasm(&source, &destination)?;
    }

    Ok(PathBuf::from("build").join(
        destination
            .file_name()
            .ok_or_else(|| anyhow!("materialized wasm filename is missing"))?,
    ))
}

fn copy_materialized_wasm(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::copy(source, destination).with_context(|| {
        format!(
            "failed to copy wasm from {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

pub fn resolve_manifest_output_path(env: Option<&str>) -> anyhow::Result<PathBuf> {
    match env {
        Some(env_name) => {
            validate_env_name(env_name)?;
            Ok(PathBuf::from(format!("build/manifest.{env_name}.json")))
        }
        None => Ok(PathBuf::from("build/manifest.json")),
    }
}

pub fn default_target_name() -> &'static str {
    DEFAULT_TARGET_NAME
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-cli-build-tests-{}-{}-{}",
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

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn write_imago_toml(root: &Path, body: &str) {
        write_file(&root.join("imago.toml"), body.as_bytes());
    }

    fn read_manifest(root: &Path, relative_path: &Path) -> Manifest {
        let bytes = fs::read(root.join(relative_path)).expect("manifest should exist");
        serde_json::from_slice(&bytes).expect("manifest json should parse")
    }

    fn assert_hashed_main_path(manifest: &Manifest, service_name: &str) -> PathBuf {
        assert!(
            !manifest.main.contains('/'),
            "manifest.main must not contain slash: {}",
            manifest.main
        );
        assert!(
            !manifest.main.contains('\\'),
            "manifest.main must not contain backslash: {}",
            manifest.main
        );

        assert!(
            !manifest.main.starts_with("build/"),
            "manifest.main must not start with build/: {}",
            manifest.main
        );

        let expected_suffix = format!("-{service_name}.wasm");
        assert!(
            manifest.main.ends_with(&expected_suffix),
            "manifest.main must end with {}: {}",
            expected_suffix,
            manifest.main
        );

        let stem = manifest
            .main
            .strip_suffix(&expected_suffix)
            .expect("expected suffix should exist");
        assert_eq!(stem.len(), 64, "sha256 prefix must have 64 hex chars");
        assert!(
            stem.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "sha256 prefix must be lowercase hex: {}",
            stem
        );

        PathBuf::from("build").join(&manifest.main)
    }

    #[test]
    fn build_generates_env_manifest_only_when_env_is_specified() {
        let root = new_temp_dir("env-manifest-only");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"

[env.prod]
type = "http"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join(".env.prod"), b"SECRET_TOKEN=abc\n");

        let output = build_project(Some("prod"), "default", &root).expect("build should succeed");
        assert_eq!(
            output.manifest_path,
            PathBuf::from("build/manifest.prod.json")
        );
        assert!(!root.join("build/manifest.json").exists());

        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.app_type, "http");
        assert_eq!(
            manifest.secrets.get("SECRET_TOKEN"),
            Some(&"abc".to_string())
        );
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        assert!(root.join(&hashed_main).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_fails_when_env_file_is_missing() {
        let root = new_temp_dir("env-missing");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"

[env.prod]
type = "cli"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(Some("prod"), "default", &root)
            .expect_err("missing env file should fail");
        assert!(err.to_string().contains(".env.prod"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn env_override_replaces_top_level_table() {
        let root = new_temp_dir("env-override");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[vars]
A = "1"
B = "2"

[target.default]
remote = "127.0.0.1:4443"

[env.prod.vars]
C = "3"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join(".env.prod"), b"\n");

        let output = build_project(Some("prod"), "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.vars.get("C"), Some(&"3".to_string()));
        assert_eq!(manifest.vars.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn supports_shell_build_command() {
        let root = new_temp_dir("shell-command");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[build]
command = "mkdir -p build && printf shell > build/app.wasm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let output = build_project(None, "default", &root).expect("build should succeed");
        assert!(root.join("build/app.wasm").exists());
        let manifest = read_manifest(&root, &output.manifest_path);
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        assert_eq!(
            fs::read(root.join("build/app.wasm")).unwrap(),
            fs::read(root.join(hashed_main)).unwrap()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn supports_argv_build_command() {
        let root = new_temp_dir("argv-command");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[build]
command = ["sh", "-c", "mkdir -p build && printf argv > build/app.wasm"]

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        assert_eq!(
            fs::read(root.join("build/app.wasm")).unwrap(),
            fs::read(root.join(hashed_main)).unwrap()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_command_receives_env_file_values() {
        let root = new_temp_dir("command-env-injection");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[build]
command = ["sh", "-c", "mkdir -p build && printf \"$BUILD_TOKEN\" > build/app.wasm"]

[target.default]
remote = "127.0.0.1:4443"

[env.prod]
type = "cli"
"#,
        );
        write_file(&root.join(".env.prod"), b"BUILD_TOKEN=token123\n");

        let _ = build_project(Some("prod"), "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, Path::new("build/manifest.prod.json"));
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        let wasm = fs::read(root.join(hashed_main)).expect("wasm should exist");
        assert_eq!(wasm, b"token123");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn no_build_command_requires_existing_main_file() {
        let root = new_temp_dir("no-command-main");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let err = build_project(None, "default", &root).expect_err("missing main file should fail");
        assert!(err.to_string().contains("main file is not accessible"));

        write_file(&root.join("build/app.wasm"), b"wasm-a");
        let output = build_project(None, "default", &root).expect("build should succeed");
        assert_eq!(output.manifest_path, PathBuf::from("build/manifest.json"));
        let manifest = read_manifest(&root, &output.manifest_path);
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        assert!(root.join(hashed_main).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn does_not_run_build_command_when_required_config_is_invalid() {
        let root = new_temp_dir("command-not-run-on-invalid-config");
        write_imago_toml(
            &root,
            r#"
main = "build/app.wasm"
type = "cli"

[build]
command = "mkdir -p build && printf side-effect > build/side-effect.txt"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let err = build_project(None, "default", &root)
            .expect_err("missing required key should fail before build.command");
        assert!(
            err.to_string()
                .contains("imago.toml missing required key: name")
        );
        assert!(!root.join("build/side-effect.txt").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_target_contains_only_remote_and_server_name() {
        let root = new_temp_dir("target-shape");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
server_name = "localhost"
ca_cert = "certs/ca.crt"
client_cert = "certs/client.crt"
client_key = "certs/client.key"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);

        assert_eq!(
            manifest.target.get("remote"),
            Some(&"127.0.0.1:4443".to_string())
        );
        assert_eq!(
            manifest.target.get("server_name"),
            Some(&"localhost".to_string())
        );
        assert_eq!(manifest.target.len(), 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_includes_bindings_from_imago_toml() {
        let root = new_temp_dir("manifest-bindings");
        write_imago_toml(
            &root,
            r#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"

[[bindings]]
target = "svc-b"
wit = "yieldspace:svc/invoke"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.bindings.len(), 1);
        assert_eq!(manifest.bindings[0].target, "svc-b");
        assert_eq!(manifest.bindings[0].wit, "yieldspace:svc/invoke");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_invalid_bindings_shape() {
        let root = new_temp_dir("manifest-bindings-invalid-shape");
        write_imago_toml(
            &root,
            r#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"

[[bindings]]
target = "svc-b"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(None, "default", &root)
            .expect_err("build must fail when bindings.wit is missing");
        assert!(err.to_string().contains("bindings[0].wit"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_cert_paths_are_resolved_relative_to_project_root() {
        let root = new_temp_dir("target-cert-relative");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
ca_cert = "certs/ca.crt"
client_cert = "certs/client.crt"
client_key = "certs/client.key"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project(None, "default", &root).expect("build should succeed");
        assert_eq!(output.target.ca_cert, Some(root.join("certs/ca.crt")));
        assert_eq!(
            output.target.client_cert,
            Some(root.join("certs/client.crt"))
        );
        assert_eq!(
            output.target.client_key,
            Some(root.join("certs/client.key"))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_cert_paths_allow_absolute_values() {
        let root = new_temp_dir("target-cert-absolute");
        let abs_ca = root.join("abs-ca.crt");
        let abs_client_cert = root.join("abs-client.crt");
        let abs_client_key = root.join("abs-client.key");
        write_imago_toml(
            &root,
            &format!(
                r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
ca_cert = "{}"
client_cert = "{}"
client_key = "{}"
"#,
                abs_ca.display(),
                abs_client_cert.display(),
                abs_client_key.display()
            ),
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project(None, "default", &root).expect("build should succeed");
        assert_eq!(output.target.ca_cert, Some(abs_ca));
        assert_eq!(output.target.client_cert, Some(abs_client_cert));
        assert_eq!(output.target.client_key, Some(abs_client_key));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_cert_path_rejects_parent_traversal() {
        let root = new_temp_dir("target-cert-dotdot");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
ca_cert = "../secrets/ca.crt"
"#,
        );

        let err = build_project(None, "default", &root)
            .expect_err("target cert path with parent traversal must fail");
        assert!(
            err.to_string()
                .contains("target key 'ca_cert' must not contain path traversal")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_cert_path_rejects_backslashes() {
        let root = new_temp_dir("target-cert-backslash");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
ca_cert = "certs\\ca.crt"
"#,
        );

        let err =
            build_project(None, "default", &root).expect_err("backslash path must be rejected");
        assert!(
            err.to_string()
                .contains("target key 'ca_cert' must not contain backslashes")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_cert_path_rejects_windows_prefix() {
        let root = new_temp_dir("target-cert-windows-prefix");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
ca_cert = "C:/certs/ca.crt"
"#,
        );

        let err = build_project(None, "default", &root)
            .expect_err("windows-prefixed cert path must be rejected");
        assert!(
            err.to_string()
                .contains("target key 'ca_cert' must not be windows-prefixed")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn secrets_are_loaded_only_from_env_file() {
        let root = new_temp_dir("secrets-source");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[secrets]
FROM_TOML = "nope"

[target.default]
remote = "127.0.0.1:4443"

[env.prod]
type = "cli"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join(".env.prod"), b"FROM_ENV=ok\n");

        let output = build_project(Some("prod"), "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);

        assert_eq!(manifest.secrets.get("FROM_ENV"), Some(&"ok".to_string()));
        assert_eq!(manifest.secrets.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_hash_changes_when_asset_changes() {
        let root = new_temp_dir("hash-assets");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[assets]]
path = "assets/message.txt"
mount = "/app/message.txt"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join("assets/message.txt"), b"hello");

        let first = build_project(None, "default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);

        write_file(&root.join("assets/message.txt"), b"hello-updated");

        let second = build_project(None, "default", &root).expect("second build should succeed");
        let second_manifest = read_manifest(&root, &second.manifest_path);

        assert_ne!(first_manifest.hash.value, second_manifest.hash.value);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_wasm_is_reused_when_source_hash_is_unchanged() {
        let root = new_temp_dir("reuse-materialized");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let first = build_project(None, "default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);
        let first_main = assert_hashed_main_path(&first_manifest, "svc");

        let second = build_project(None, "default", &root).expect("second build should succeed");
        let second_manifest = read_manifest(&root, &second.manifest_path);
        let second_main = assert_hashed_main_path(&second_manifest, "svc");

        assert_eq!(first_main, second_main);
        assert!(root.join(first_main).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_wasm_path_changes_when_source_hash_changes() {
        let root = new_temp_dir("rotate-materialized");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let first = build_project(None, "default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);
        let first_main = assert_hashed_main_path(&first_manifest, "svc");

        write_file(&root.join("build/app.wasm"), b"wasm-b");

        let second = build_project(None, "default", &root).expect("second build should succeed");
        let second_manifest = read_manifest(&root, &second.manifest_path);
        let second_main = assert_hashed_main_path(&second_manifest, "svc");

        assert_ne!(first_main, second_main);
        assert!(root.join(first_main).exists());
        assert!(root.join(second_main).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_wasm_is_rewritten_when_existing_file_is_corrupted() {
        let root = new_temp_dir("rewrite-corrupted-materialized");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let first = build_project(None, "default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);
        let first_main = assert_hashed_main_path(&first_manifest, "svc");

        write_file(&root.join(&first_main), b"tampered");

        let second = build_project(None, "default", &root).expect("second build should succeed");
        let second_manifest = read_manifest(&root, &second.manifest_path);
        let second_main = assert_hashed_main_path(&second_manifest, "svc");

        assert_eq!(first_main, second_main);
        assert_eq!(
            fs::read(root.join("build/app.wasm")).unwrap(),
            fs::read(root.join(second_main)).unwrap()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compute_sha256_hex_matches_known_digest() {
        let root = new_temp_dir("sha256-known-digest");
        let path = root.join("payload.bin");
        let bytes = b"known-bytes-for-hash";
        write_file(&path, bytes);

        let actual = compute_sha256_hex(&path).expect("sha256 should be computed");
        let expected = hex::encode(Sha256::digest(bytes));
        assert_eq!(actual, expected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_requires_deploy_credentials_only_for_deploy_phase() {
        let target = TargetConfig {
            remote: "127.0.0.1:4443".to_string(),
            server_name: Some("localhost".to_string()),
            ca_cert: None,
            client_cert: Some(PathBuf::from("certs/client.crt")),
            client_key: Some(PathBuf::from("certs/client.key")),
        };

        let err = target
            .require_deploy_credentials()
            .expect_err("missing cert should fail");
        assert!(err.to_string().contains("ca_cert"));
    }

    #[test]
    fn resolve_manifest_output_path_follows_env_rule() {
        assert_eq!(
            resolve_manifest_output_path(None).expect("default manifest path should resolve"),
            PathBuf::from("build/manifest.json")
        );
        assert_eq!(
            resolve_manifest_output_path(Some("prod"))
                .expect("env manifest path should resolve for valid env"),
            PathBuf::from("build/manifest.prod.json")
        );
    }

    #[test]
    fn resolve_manifest_output_path_rejects_path_traversal_env() {
        let err = resolve_manifest_output_path(Some("../../../outside"))
            .expect_err("path traversal env must be rejected");
        assert!(err.to_string().contains("invalid path characters"));
    }

    #[test]
    fn build_rejects_service_name_containing_path_traversal_sequence() {
        let root = new_temp_dir("service-name-dotdot");
        write_imago_toml(
            &root,
            r#"
name = "svc..bad"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(None, "default", &root)
            .expect_err("service name containing path traversal must fail");
        assert!(
            err.to_string()
                .contains("name contains invalid path characters")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_invalid_env_name_before_manifest_write() {
        let root = new_temp_dir("invalid-env-name");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"

[env."../../../outside"]
type = "cli"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(Some("../../../outside"), "default", &root)
            .expect_err("invalid env name must fail before any manifest write");
        assert!(
            err.to_string()
                .contains("env name contains invalid path characters")
        );
        assert!(!root.join("build/manifest.json").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_still_succeeds_for_valid_env_name() {
        let root = new_temp_dir("valid-env-name");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"

[env.prod]
type = "http"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join(".env.prod"), b"TOKEN=ok\n");

        let output = build_project(Some("prod"), "default", &root)
            .expect("valid env name should continue to work");
        assert_eq!(
            output.manifest_path,
            PathBuf::from("build/manifest.prod.json")
        );

        let _ = fs::remove_dir_all(root);
    }
}
