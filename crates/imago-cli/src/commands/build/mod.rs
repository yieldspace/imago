use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    net::IpAddr,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use toml::Value as TomlValue;

use crate::{
    cli::BuildArgs,
    commands::{
        CommandResult, dependency_cache, plugin_sources,
        shared::dependency::{DependencyResolver, StandardDependencyResolver},
    },
};

mod validation;

const DEFAULT_TARGET_NAME: &str = "default";
const DEFAULT_HTTP_MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_HTTP_MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_RESTART_POLICY: &str = "never";
const RESTART_POLICY_ON_FAILURE: &str = "on-failure";
const RESTART_POLICY_ALWAYS: &str = "always";
const RESTART_POLICY_UNLESS_STOPPED: &str = "unless-stopped";

#[derive(Debug, Clone)]
pub struct TargetConfig {
    pub remote: String,
    pub server_name: Option<String>,
    pub client_key: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DeployTargetConfig {
    pub remote: String,
    pub server_name: Option<String>,
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
        let client_key = self
            .client_key
            .clone()
            .ok_or_else(|| anyhow!("target is missing required key: client_key"))?;

        Ok(DeployTargetConfig {
            remote: self.remote.clone(),
            server_name: self.server_name.clone(),
            client_key,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BuildOutput {
    pub manifest_path: PathBuf,
    pub manifest_bytes: Vec<u8>,
    pub target: TargetConfig,
    pub restart_policy: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    http: Option<ManifestHttp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    socket: Option<ManifestSocket>,
    dependencies: Vec<ManifestDependency>,
    #[serde(default, skip_serializing_if = "ManifestCapabilityPolicy::is_empty")]
    capabilities: ManifestCapabilityPolicy,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ManifestDependencyKind {
    Native,
    Wasm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ManifestDependencyComponent {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDependencySource {
    pub source: String,
    pub registry: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDependencyComponent {
    pub source: String,
    pub registry: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ManifestCapabilityPolicy {
    #[serde(default)]
    pub privileged: bool,
    #[serde(default)]
    pub deps: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub wasi: BTreeMap<String, Vec<String>>,
}

impl ManifestCapabilityPolicy {
    pub(crate) fn is_empty(&self) -> bool {
        !self.privileged && self.deps.is_empty() && self.wasi.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ManifestDependency {
    pub name: String,
    pub version: String,
    pub kind: ManifestDependencyKind,
    pub wit: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<ManifestDependencyComponent>,
    #[serde(default, skip_serializing_if = "ManifestCapabilityPolicy::is_empty")]
    pub capabilities: ManifestCapabilityPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDependency {
    pub name: String,
    pub version: String,
    pub kind: ManifestDependencyKind,
    pub wit: ProjectDependencySource,
    pub requires: Vec<String>,
    pub component: Option<ProjectDependencyComponent>,
    pub capabilities: ManifestCapabilityPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestHttp {
    port: u16,
    max_body_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ManifestSocketProtocol {
    #[serde(rename = "udp")]
    Udp,
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "both")]
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ManifestSocketDirection {
    #[serde(rename = "inbound")]
    Inbound,
    #[serde(rename = "outbound")]
    Outbound,
    #[serde(rename = "both")]
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestSocket {
    protocol: ManifestSocketProtocol,
    direction: ManifestSocketDirection,
    listen_addr: String,
    listen_port: u16,
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

pub fn load_target_config(
    env: Option<&str>,
    target_name: &str,
    project_root: &Path,
) -> anyhow::Result<TargetConfig> {
    if let Some(env_name) = env {
        validate_env_name(env_name)?;
    }
    let root = load_resolved_toml(project_root, env)?;
    parse_target(&root, target_name, project_root)
}

pub fn load_service_name(env: Option<&str>, project_root: &Path) -> anyhow::Result<String> {
    if let Some(env_name) = env {
        validate_env_name(env_name)?;
    }
    let root = load_resolved_toml(project_root, env)?;
    let name = required_string(&root, "name")?;
    validate_service_name(&name)?;
    Ok(name)
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
    let http = parse_http_section(&root, &app_type)?;
    let socket = parse_socket_section(&root, &app_type)?;
    let restart_policy = parse_restart_policy(&root)?;

    let vars = parse_string_table(root.get("vars"), "vars")?;
    let bindings = parse_bindings(root.get("bindings"))?;
    let project_dependencies = parse_project_dependencies(root.get("dependencies"))?;
    if !project_dependencies.is_empty() {
        dependency_cache::hydrate_project_wit_deps(project_root, &project_dependencies)
            .context("failed to hydrate dependency cache")?;
    }
    let capabilities = parse_root_capabilities(&root)?;
    let dependency_resolver = StandardDependencyResolver;
    let dependencies = dependency_resolver
        .resolve_manifest_dependencies_from_lock(project_root, &project_dependencies)?;
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
        http,
        socket,
        dependencies,
        capabilities,
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
        restart_policy,
    })
}

fn load_resolved_toml(project_root: &Path, env: Option<&str>) -> anyhow::Result<toml::Table> {
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

    if let Some(runtime) = root.get("runtime").and_then(TomlValue::as_table)
        && runtime.get("restart_policy").is_some()
    {
        return Err(anyhow!(
            "runtime.restart_policy is no longer supported; use top-level restart"
        ));
    }

    Ok(root)
}

fn parse_restart_policy(root: &toml::Table) -> anyhow::Result<String> {
    let Some(raw) = root.get("restart") else {
        return Ok(DEFAULT_RESTART_POLICY.to_string());
    };
    let value = raw
        .as_str()
        .ok_or_else(|| anyhow!("imago.toml key 'restart' must be a string"))?
        .trim()
        .to_string();
    if !is_supported_restart_policy(&value) {
        return Err(anyhow!(
            "imago.toml key 'restart' must be one of: never, on-failure, always, unless-stopped (got: {value})"
        ));
    }
    Ok(value)
}

fn is_supported_restart_policy(value: &str) -> bool {
    matches!(
        value,
        DEFAULT_RESTART_POLICY
            | RESTART_POLICY_ON_FAILURE
            | RESTART_POLICY_ALWAYS
            | RESTART_POLICY_UNLESS_STOPPED
    )
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

pub(crate) fn validate_service_name(name: &str) -> anyhow::Result<()> {
    validation::validate_service_name(name)
}

fn validate_env_name(env_name: &str) -> anyhow::Result<()> {
    validation::validate_env_name(env_name)
}

fn validate_app_type(app_type: &str) -> anyhow::Result<()> {
    validation::validate_app_type(app_type)
}

fn parse_http_section(root: &toml::Table, app_type: &str) -> anyhow::Result<Option<ManifestHttp>> {
    let http = root.get("http");

    if app_type != "http" {
        if http.is_some() {
            return Err(anyhow!(
                "http section is only allowed when type is \"http\""
            ));
        }
        return Ok(None);
    }

    let table = http
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("type=\"http\" requires [http] table"))?;
    let raw_port = table
        .get("port")
        .and_then(TomlValue::as_integer)
        .ok_or_else(|| anyhow!("http.port is required when type=\"http\""))?;
    let port = u16::try_from(raw_port)
        .map_err(|_| anyhow!("http.port must be in range 1..=65535 (got {raw_port})"))?;
    if port == 0 {
        return Err(anyhow!("http.port must be in range 1..=65535 (got 0)"));
    }

    let max_body_bytes = match table.get("max_body_bytes") {
        Some(value) => {
            let raw = value.as_integer().ok_or_else(|| {
                anyhow!("http.max_body_bytes must be in range 1..={MAX_HTTP_MAX_BODY_BYTES}")
            })?;
            let value = u64::try_from(raw).map_err(|_| {
                anyhow!(
                    "http.max_body_bytes must be in range 1..={} (got {raw})",
                    MAX_HTTP_MAX_BODY_BYTES
                )
            })?;
            if value == 0 || value > MAX_HTTP_MAX_BODY_BYTES {
                return Err(anyhow!(
                    "http.max_body_bytes must be in range 1..={} (got {value})",
                    MAX_HTTP_MAX_BODY_BYTES
                ));
            }
            value
        }
        None => DEFAULT_HTTP_MAX_BODY_BYTES,
    };

    Ok(Some(ManifestHttp {
        port,
        max_body_bytes,
    }))
}

fn parse_socket_section(
    root: &toml::Table,
    app_type: &str,
) -> anyhow::Result<Option<ManifestSocket>> {
    let socket = root.get("socket");

    if app_type != "socket" {
        if socket.is_some() {
            return Err(anyhow!(
                "socket section is only allowed when type is \"socket\""
            ));
        }
        return Ok(None);
    }

    let table = socket
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("type=\"socket\" requires [socket] table"))?;

    let protocol_raw = table
        .get("protocol")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("socket.protocol is required when type=\"socket\""))?;
    let protocol = match protocol_raw {
        "udp" => ManifestSocketProtocol::Udp,
        "tcp" => ManifestSocketProtocol::Tcp,
        "both" => ManifestSocketProtocol::Both,
        _ => {
            return Err(anyhow!(
                "socket.protocol must be one of: udp, tcp, both (got: {protocol_raw})"
            ));
        }
    };

    let direction_raw = table
        .get("direction")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("socket.direction is required when type=\"socket\""))?;
    let direction = match direction_raw {
        "inbound" => ManifestSocketDirection::Inbound,
        "outbound" => ManifestSocketDirection::Outbound,
        "both" => ManifestSocketDirection::Both,
        _ => {
            return Err(anyhow!(
                "socket.direction must be one of: inbound, outbound, both (got: {direction_raw})"
            ));
        }
    };

    let listen_addr = table
        .get("listen_addr")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("socket.listen_addr is required when type=\"socket\""))?
        .trim()
        .to_string();
    if listen_addr.is_empty() {
        return Err(anyhow!(
            "socket.listen_addr must be a valid IP address (got empty value)"
        ));
    }
    listen_addr.parse::<IpAddr>().map_err(|err| {
        anyhow!("socket.listen_addr must be a valid IP address (got '{listen_addr}'): {err}")
    })?;

    let raw_port = table
        .get("listen_port")
        .and_then(TomlValue::as_integer)
        .ok_or_else(|| anyhow!("socket.listen_port is required when type=\"socket\""))?;
    let listen_port = u16::try_from(raw_port)
        .map_err(|_| anyhow!("socket.listen_port must be in range 1..=65535 (got {raw_port})"))?;
    if listen_port == 0 {
        return Err(anyhow!(
            "socket.listen_port must be in range 1..=65535 (got 0)"
        ));
    }

    Ok(Some(ManifestSocket {
        protocol,
        direction,
        listen_addr,
        listen_port,
    }))
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

pub(crate) fn load_project_dependencies(
    project_root: &Path,
) -> anyhow::Result<Vec<ProjectDependency>> {
    let root = load_resolved_toml(project_root, None)?;
    parse_project_dependencies(root.get("dependencies"))
}

fn parse_project_dependencies(value: Option<&TomlValue>) -> anyhow::Result<Vec<ProjectDependency>> {
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

        let name = table
            .get("name")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("dependencies[{index}].name must be a string"))?
            .trim()
            .to_string();
        validate_dependency_package_name(&name)
            .map_err(|err| anyhow!("dependencies[{index}].name is invalid: {err}"))?;
        if !names.insert(name.clone()) {
            return Err(anyhow!(
                "dependencies contains duplicate dependency name: {name}"
            ));
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

        let wit = parse_dependency_wit_source(table.get("wit"), index, &name, &version)
            .with_context(|| {
                format!("failed to parse dependencies[{index}].wit source configuration")
            })?;

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
                if component_table.contains_key("path") {
                    return Err(anyhow!(
                        "dependencies[{index}].component.path is no longer supported; use dependencies[{index}].component.source"
                    ));
                }
                for key in component_table.keys() {
                    if !matches!(key.as_str(), "source" | "registry" | "sha256") {
                        return Err(anyhow!(
                            "dependencies[{index}].component.{key} is not supported"
                        ));
                    }
                }

                let source = component_table
                    .get("source")
                    .and_then(TomlValue::as_str)
                    .ok_or_else(|| {
                        anyhow!("dependencies[{index}].component.source must be a string")
                    })?
                    .trim()
                    .to_string();
                if source.is_empty() {
                    return Err(anyhow!(
                        "dependencies[{index}].component.source must not be empty"
                    ));
                }
                plugin_sources::validate_component_source(
                    &source,
                    &format!("dependencies[{index}].component.source"),
                )?;

                let registry = match component_table.get("registry") {
                    None => None,
                    Some(value) => Some(
                        value
                            .as_str()
                            .ok_or_else(|| {
                                anyhow!("dependencies[{index}].component.registry must be a string")
                            })?
                            .trim()
                            .to_string(),
                    ),
                };
                let registry = plugin_sources::normalize_registry_for_source(
                    &source,
                    registry.as_deref(),
                    &format!("dependencies[{index}].component"),
                )?;

                let sha256 = match component_table.get("sha256") {
                    None => None,
                    Some(value) => {
                        let sha = value
                            .as_str()
                            .ok_or_else(|| {
                                anyhow!("dependencies[{index}].component.sha256 must be a string")
                            })?
                            .trim()
                            .to_string();
                        if sha.is_empty() {
                            return Err(anyhow!(
                                "dependencies[{index}].component.sha256 must not be empty"
                            ));
                        }
                        plugin_sources::validate_sha256_hex(
                            &sha,
                            &format!("dependencies[{index}].component.sha256"),
                        )?;
                        Some(sha)
                    }
                };

                Some(ProjectDependencyComponent {
                    source,
                    registry,
                    sha256,
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

fn parse_dependency_wit_source(
    value: Option<&TomlValue>,
    index: usize,
    name: &str,
    version: &str,
) -> anyhow::Result<ProjectDependencySource> {
    let default_source = format!("warg://{name}@{version}");
    let default_registry = Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string());

    let (source, registry) = match value {
        None => (default_source, default_registry),
        Some(TomlValue::String(text)) => {
            let source = text.trim().to_string();
            if source.is_empty() {
                return Err(anyhow!("dependencies[{index}].wit must not be empty"));
            }
            plugin_sources::validate_wit_source(&source, &format!("dependencies[{index}].wit"))?;
            let registry = plugin_sources::normalize_registry_for_source(
                &source,
                None,
                &format!("dependencies[{index}].wit"),
            )?;
            (source, registry)
        }
        Some(TomlValue::Table(table)) => {
            for key in table.keys() {
                if !matches!(key.as_str(), "source" | "registry") {
                    return Err(anyhow!("dependencies[{index}].wit.{key} is not supported"));
                }
            }

            let source = table
                .get("source")
                .and_then(TomlValue::as_str)
                .ok_or_else(|| anyhow!("dependencies[{index}].wit.source must be a string"))?
                .trim()
                .to_string();
            if source.is_empty() {
                return Err(anyhow!(
                    "dependencies[{index}].wit.source must not be empty"
                ));
            }
            plugin_sources::validate_wit_source(
                &source,
                &format!("dependencies[{index}].wit.source"),
            )?;

            let registry = match table.get("registry") {
                None => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            anyhow!("dependencies[{index}].wit.registry must be a string")
                        })?
                        .trim()
                        .to_string(),
                ),
            };
            let registry = plugin_sources::normalize_registry_for_source(
                &source,
                registry.as_deref(),
                &format!("dependencies[{index}].wit"),
            )?;
            (source, registry)
        }
        Some(_) => {
            return Err(anyhow!(
                "dependencies[{index}].wit must be a string or a table"
            ));
        }
    };

    Ok(ProjectDependencySource { source, registry })
}

fn parse_root_capabilities(root: &toml::Table) -> anyhow::Result<ManifestCapabilityPolicy> {
    if root.contains_key("capabilirties") {
        return Err(anyhow!("unknown key 'capabilirties'; use 'capabilities'"));
    }
    parse_capability_policy(root.get("capabilities"), "capabilities")
}

fn parse_capability_policy(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<ManifestCapabilityPolicy> {
    let Some(value) = value else {
        return Ok(ManifestCapabilityPolicy::default());
    };
    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("{field_name} must be a table"))?;

    for key in table.keys() {
        if !matches!(key.as_str(), "privileged" | "deps" | "wasi") {
            return Err(anyhow!("{field_name}.{key} is not supported"));
        }
    }

    let privileged = match table.get("privileged") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| anyhow!("{field_name}.privileged must be a boolean"))?,
    };

    let deps = parse_capability_rule_table(table.get("deps"), &format!("{field_name}.deps"))?;
    let wasi = parse_capability_rule_table(table.get("wasi"), &format!("{field_name}.wasi"))?;

    Ok(ManifestCapabilityPolicy {
        privileged,
        deps,
        wasi,
    })
}

fn parse_capability_rule_table(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("{field_name} must be a table"))?;

    let mut normalized = BTreeMap::new();
    for (key, value) in table {
        if key.trim().is_empty() {
            return Err(anyhow!("{field_name} contains an empty key"));
        }
        let rules = parse_capability_rule_list(value, &format!("{field_name}.{key}"))?;
        if !rules.is_empty() {
            normalized.insert(key.clone(), rules);
        }
    }
    Ok(normalized)
}

fn parse_capability_rule_list(value: &TomlValue, field_name: &str) -> anyhow::Result<Vec<String>> {
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("{field_name} must be an array of strings"))?;
    let mut rules = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let text = value
            .as_str()
            .ok_or_else(|| anyhow!("{field_name}[{index}] must be a string"))?
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(anyhow!("{field_name}[{index}] must not be empty"));
        }
        rules.push(text);
    }
    Ok(normalize_string_list(rules))
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            set.insert(value.to_string());
        }
    }
    set.into_iter().collect()
}

fn validate_dependency_package_name(name: &str) -> anyhow::Result<()> {
    validation::validate_dependency_package_name(name)
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

fn parse_target(
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
    if target_table.contains_key("ca_cert") {
        return Err(anyhow!(
            "target key 'ca_cert' is no longer supported; use target.<name>.client_key with RPK+TOFU"
        ));
    }
    if target_table.contains_key("client_cert") {
        return Err(anyhow!(
            "target key 'client_cert' is no longer supported; use target.<name>.client_key with RPK+TOFU"
        ));
    }
    if target_table.contains_key("known_hosts") {
        return Err(anyhow!(
            "target key 'known_hosts' is no longer supported; CLI always uses ~/.imago/known_hosts"
        ));
    }
    let client_key = optional_target_credential_path(target_table, "client_key", project_root)?;

    Ok(TargetConfig {
        remote,
        server_name,
        client_key,
    })
}

fn optional_string(table: &toml::Table, key: &str) -> anyhow::Result<Option<String>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("target key '{}' must be a string", key))?
        .to_string();
    Ok(Some(text))
}

fn optional_target_credential_path(
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
    let normalized = normalize_target_credential_path(text, key)?;
    if normalized.is_absolute() {
        Ok(Some(normalized))
    } else {
        Ok(Some(project_root.join(normalized)))
    }
}

fn normalize_target_credential_path(raw: &str, key: &str) -> anyhow::Result<PathBuf> {
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

pub(crate) fn compute_path_digest_hex(path: &Path) -> anyhow::Result<String> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read path for digest: {}", path.display()))?;
    if metadata.is_file() {
        return compute_sha256_hex(path);
    }
    if !metadata.is_dir() {
        return Err(anyhow!("path is not file or directory: {}", path.display()));
    }

    let mut stack = vec![path.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory for digest: {}", dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read directory entry while hashing {}",
                    dir.display()
                )
            })?;
            let entry_path = entry.path();
            let entry_metadata = entry
                .metadata()
                .with_context(|| format!("failed to read metadata for {}", entry_path.display()))?;
            if entry_metadata.is_dir() {
                stack.push(entry_path);
            } else if entry_metadata.is_file() {
                files.push(entry_path);
            }
        }
    }
    files.sort();

    let mut hasher = Sha256::new();
    for file in files {
        let rel = file
            .strip_prefix(path)
            .with_context(|| format!("failed to relativize digest path: {}", file.display()))?;
        hasher.update(normalized_path_to_string(rel).as_bytes());
        hasher.update([0]);
        hash_file_into(&mut hasher, &file, "directory digest file")?;
        hasher.update([0]);
    }
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
    use crate::{
        cli::UpdateArgs,
        commands::{dependency_cache, update},
    };
    use imago_lockfile::{
        IMAGO_LOCK_VERSION, ImagoLock, ImagoLockDependency, ImagoLockWitPackage,
        ImagoLockWitPackageVersion,
    };

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

    fn write_imago_lock(root: &Path, lock: &ImagoLock) {
        let encoded = toml::to_string_pretty(lock).expect("lock should serialize");
        write_file(&root.join("imago.lock"), encoded.as_bytes());
    }

    fn read_manifest(root: &Path, relative_path: &Path) -> Manifest {
        let bytes = fs::read(root.join(relative_path)).expect("manifest should exist");
        serde_json::from_slice(&bytes).expect("manifest json should parse")
    }

    fn run_update(root: &Path) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime for update helper should be created");
        let result = runtime.block_on(update::run_with_project_root(UpdateArgs {}, root));
        assert_eq!(
            result.exit_code, 0,
            "imago update should succeed before build tests: {:?}",
            result.stderr
        );
    }

    fn copy_tree(source: &Path, destination: &Path) {
        let metadata = fs::metadata(source)
            .unwrap_or_else(|_| panic!("source must exist: {}", source.display()));
        if metadata.is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).expect("destination parent should be created");
            }
            fs::copy(source, destination).unwrap_or_else(|_| {
                panic!(
                    "failed to copy source file {} -> {}",
                    source.display(),
                    destination.display()
                )
            });
            return;
        }
        if !metadata.is_dir() {
            panic!("source is not file/dir: {}", source.display());
        }
        fs::create_dir_all(destination).expect("destination dir should be created");
        for entry in fs::read_dir(source).expect("source directory should be readable") {
            let entry = entry.expect("source directory entry should be readable");
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            copy_tree(&source_path, &destination_path);
        }
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

[env.prod.http]
port = 18080
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
        assert_eq!(manifest.http.as_ref().map(|v| v.port), Some(18080));
        assert_eq!(
            manifest.http.as_ref().map(|v| v.max_body_bytes),
            Some(DEFAULT_HTTP_MAX_BODY_BYTES)
        );
        assert_eq!(
            manifest.secrets.get("SECRET_TOKEN"),
            Some(&"abc".to_string())
        );
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        assert!(root.join(&hashed_main).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_succeeds_for_http_type_with_http_port() {
        let root = new_temp_dir("http-type-valid");
        write_imago_toml(
            &root,
            r#"
name = "svc-http"
main = "build/app.wasm"
type = "http"

[http]
port = 18080

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-http");

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.app_type, "http");
        assert_eq!(manifest.http.as_ref().map(|v| v.port), Some(18080));
        assert_eq!(
            manifest.http.as_ref().map(|v| v.max_body_bytes),
            Some(DEFAULT_HTTP_MAX_BODY_BYTES)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_succeeds_for_http_type_with_explicit_http_max_body_bytes() {
        let root = new_temp_dir("http-type-max-body-valid");
        write_imago_toml(
            &root,
            r#"
name = "svc-http"
main = "build/app.wasm"
type = "http"

[http]
port = 18080
max_body_bytes = 4096

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-http");

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.http.as_ref().map(|v| v.max_body_bytes), Some(4096));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_http_type_without_http_port() {
        let root = new_temp_dir("http-type-missing-port");
        write_imago_toml(
            &root,
            r#"
name = "svc-http"
main = "build/app.wasm"
type = "http"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-http");

        let err = build_project(None, "default", &root)
            .expect_err("build must fail when type=http has no http.port");
        assert!(err.to_string().contains("requires [http] table"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_http_section_for_non_http_type() {
        for app_type in ["cli", "socket"] {
            let root = new_temp_dir(&format!("http-section-non-http-{app_type}"));
            write_imago_toml(
                &root,
                &format!(
                    r#"
name = "svc-{app_type}"
main = "build/app.wasm"
type = "{app_type}"

[http]
port = 18080

[target.default]
remote = "127.0.0.1:4443"
"#,
                ),
            );
            write_file(&root.join("build/app.wasm"), b"wasm-cli");

            let err = build_project(None, "default", &root)
                .expect_err("build must fail when non-http type uses [http]");
            assert!(
                err.to_string()
                    .contains("http section is only allowed when type is \"http\"")
            );

            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn build_rejects_invalid_http_max_body_bytes() {
        for (suffix, max_body_bytes) in [("zero", "0"), ("too-large", "67108865")] {
            let root = new_temp_dir(&format!("http-max-body-{suffix}"));
            write_imago_toml(
                &root,
                &format!(
                    r#"
name = "svc-http"
main = "build/app.wasm"
type = "http"

[http]
port = 18080
max_body_bytes = {max_body_bytes}

[target.default]
remote = "127.0.0.1:4443"
"#
                ),
            );
            write_file(&root.join("build/app.wasm"), b"wasm-http");

            let err = build_project(None, "default", &root)
                .expect_err("invalid max_body_bytes must fail");
            assert!(err.to_string().contains("http.max_body_bytes"));

            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn build_succeeds_for_socket_type_with_socket_section() {
        let root = new_temp_dir("socket-type-valid");
        write_imago_toml(
            &root,
            r#"
name = "svc-socket"
main = "build/app.wasm"
type = "socket"

[socket]
protocol = "udp"
direction = "inbound"
listen_addr = "0.0.0.0"
listen_port = 514

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-socket");

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let socket = manifest
            .socket
            .as_ref()
            .expect("socket section should be emitted");
        assert!(matches!(socket.protocol, ManifestSocketProtocol::Udp));
        assert!(matches!(socket.direction, ManifestSocketDirection::Inbound));
        assert_eq!(socket.listen_addr, "0.0.0.0");
        assert_eq!(socket.listen_port, 514);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_socket_type_without_socket_section() {
        let root = new_temp_dir("socket-type-missing-section");
        write_imago_toml(
            &root,
            r#"
name = "svc-socket"
main = "build/app.wasm"
type = "socket"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-socket");

        let err = build_project(None, "default", &root)
            .expect_err("type=socket without [socket] must fail");
        assert!(
            err.to_string()
                .contains("type=\"socket\" requires [socket] table")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_socket_section_for_non_socket_type() {
        for (app_type, app_type_extra) in [("cli", ""), ("http", "[http]\nport = 18080\n")] {
            let root = new_temp_dir(&format!("socket-section-non-socket-{app_type}"));
            write_imago_toml(
                &root,
                &format!(
                    r#"
name = "svc-{app_type}"
main = "build/app.wasm"
type = "{app_type}"

{app_type_extra}

[socket]
protocol = "udp"
direction = "inbound"
listen_addr = "0.0.0.0"
listen_port = 514

[target.default]
remote = "127.0.0.1:4443"
"#
                ),
            );
            write_file(&root.join("build/app.wasm"), b"wasm-a");

            let err = build_project(None, "default", &root)
                .expect_err("build must fail when non-socket type uses [socket]");
            assert!(
                err.to_string()
                    .contains("socket section is only allowed when type is \"socket\"")
            );

            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn build_rejects_invalid_socket_section_values() {
        let invalid_cases = [
            (
                "bad-protocol",
                r#"
name = "svc"
main = "build/app.wasm"
type = "socket"

[socket]
protocol = "icmp"
direction = "inbound"
listen_addr = "0.0.0.0"
listen_port = 514

[target.default]
remote = "127.0.0.1:4443"
"#,
                "socket.protocol must be one of",
            ),
            (
                "bad-direction",
                r#"
name = "svc"
main = "build/app.wasm"
type = "socket"

[socket]
protocol = "udp"
direction = "sideways"
listen_addr = "0.0.0.0"
listen_port = 514

[target.default]
remote = "127.0.0.1:4443"
"#,
                "socket.direction must be one of",
            ),
            (
                "bad-listen-addr",
                r#"
name = "svc"
main = "build/app.wasm"
type = "socket"

[socket]
protocol = "udp"
direction = "inbound"
listen_addr = "not-an-ip"
listen_port = 514

[target.default]
remote = "127.0.0.1:4443"
"#,
                "socket.listen_addr must be a valid IP address",
            ),
            (
                "bad-listen-port",
                r#"
name = "svc"
main = "build/app.wasm"
type = "socket"

[socket]
protocol = "udp"
direction = "inbound"
listen_addr = "0.0.0.0"
listen_port = 0

[target.default]
remote = "127.0.0.1:4443"
"#,
                "socket.listen_port must be in range",
            ),
        ];

        for (suffix, body, expected) in invalid_cases {
            let root = new_temp_dir(&format!("socket-invalid-{suffix}"));
            write_imago_toml(&root, body);
            write_file(&root.join("build/app.wasm"), b"wasm-a");

            let err = build_project(None, "default", &root).expect_err("invalid socket section");
            assert!(
                err.to_string().contains(expected),
                "unexpected error for {suffix}: {err}"
            );

            let _ = fs::remove_dir_all(root);
        }
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
    fn build_rejects_capabilirties_typo_key() {
        let root = new_temp_dir("capability-typo");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[capabilirties]
privileged = true

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(None, "default", &root).expect_err("typo key must be rejected");
        assert!(err.to_string().contains("capabilirties"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_requires_imago_lock_for_dependencies() {
        let root = new_temp_dir("dependencies-lock-required");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        fs::remove_file(root.join("imago.lock")).expect("lock should be removable");

        let err = build_project(None, "default", &root)
            .expect_err("build should fail when lock is missing");
        assert!(err.to_string().contains("imago.lock is missing"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rehydrates_wit_deps_from_dependency_cache() {
        let root = new_temp_dir("dependencies-rehydrate-wit-deps");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        fs::remove_dir_all(root.join("wit/deps")).expect("wit/deps should be removable");

        build_project(None, "default", &root)
            .expect("build should succeed by hydrating wit/deps from dependency cache");
        assert!(
            root.join("wit/deps/yieldspace-plugin/example/example.wit")
                .exists(),
            "wit/deps must be rehydrated from dependency cache before lock validation"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_fails_when_dependency_cache_is_missing() {
        let root = new_temp_dir("dependencies-cache-required");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        fs::remove_dir_all(root.join(".imago/deps")).expect("dependency cache should be removable");

        let err = build_project(None, "default", &root)
            .expect_err("build should fail when dependency cache is missing");
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains(".imago/deps"),
            "unexpected error: {err:#}"
        );
        assert!(
            err_chain.contains("run `imago update`"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_dependency_name_with_absolute_path_component() {
        let root = new_temp_dir("dependency-name-absolute-path");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "/tmp/pwn"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(None, "default", &root)
            .expect_err("absolute dependency package name must fail");
        let err_text = err.to_string();
        assert!(
            err_text.contains("dependencies[0].name is invalid"),
            "unexpected error: {err_text}"
        );
        assert!(
            err_text.contains("invalid path components"),
            "unexpected error: {err_text}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_dependency_name_with_normalized_path_segments() {
        let root = new_temp_dir("dependency-name-normalized-segments");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/./example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(None, "default", &root)
            .expect_err("dependency package with normalized path segment must fail");
        let err_text = err.to_string();
        assert!(
            err_text.contains("dependencies[0].name is invalid"),
            "unexpected error: {err_text}"
        );
        assert!(
            err_text.contains("invalid path components"),
            "unexpected error: {err_text}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_dependency_requires_with_absolute_path_component() {
        let root = new_temp_dir("dependency-requires-absolute-path");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"
requires = ["/tmp/pwn"]

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(None, "default", &root)
            .expect_err("absolute dependency requirement must fail");
        let err_text = err.to_string();
        assert!(
            err_text.contains("dependencies[0].requires[0] is invalid"),
            "unexpected error: {err_text}"
        );
        assert!(
            err_text.contains("invalid path components"),
            "unexpected error: {err_text}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_imago_lock_version_mismatch() {
        let root = new_temp_dir("dependencies-lock-version-mismatch");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        let mut lock: ImagoLock = toml::from_str(
            &fs::read_to_string(root.join("imago.lock")).expect("lock should exist"),
        )
        .expect("lock should parse");
        lock.version = 2;
        write_imago_lock(&root, &lock);

        let err = build_project(None, "default", &root)
            .expect_err("build should reject unsupported lock version");
        assert!(
            err.to_string()
                .contains("imago.lock version '2' is not supported")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_wit_package_with_unknown_via_dependency() {
        let root = new_temp_dir("dependencies-transitive-via-unknown");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/yieldspace-plugin/example/example.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/test-dep/package.wit"),
            b"package test:dep; interface dep { pong: func() -> string; }\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/yieldspace-plugin/example"))
            .expect("wit digest should compute");
        let transitive_digest = format!(
            "sha256:{}",
            compute_sha256_hex(&root.join("wit/deps/test-dep/package.wit"))
                .expect("transitive digest should compute")
        );
        write_imago_lock(
            &root,
            &ImagoLock {
                version: IMAGO_LOCK_VERSION,
                dependencies: vec![ImagoLockDependency {
                    name: "yieldspace:plugin/example".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "file://registry/example.wit".to_string(),
                    wit_registry: None,
                    wit_digest: digest.clone(),
                    wit_path: "wit/deps/yieldspace-plugin/example".to_string(),
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                    resolved_at: "0".to_string(),
                }],
                wit_packages: vec![ImagoLockWitPackage {
                    name: "test:dep".to_string(),
                    registry: None,
                    versions: vec![ImagoLockWitPackageVersion {
                        requirement: "*".to_string(),
                        version: None,
                        digest: transitive_digest.clone(),
                        source: None,
                        path: "wit/deps/test-dep".to_string(),
                        via: vec!["yieldspace:plugin/other".to_string()],
                    }],
                }],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "yieldspace:plugin/example".to_string(),
            version: "0.1.0".to_string(),
            kind: "native".to_string(),
            wit_source: "file://registry/example.wit".to_string(),
            wit_registry: None,
            wit_path: "wit/deps/yieldspace-plugin/example".to_string(),
            wit_digest: digest,
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            transitive_packages: vec![dependency_cache::DependencyCacheTransitivePackage {
                name: "test:dep".to_string(),
                registry: None,
                requirement: "*".to_string(),
                version: None,
                digest: transitive_digest,
                source: None,
                path: "wit/deps/test-dep".to_string(),
            }],
        };
        let cache_root = dependency_cache::cache_entry_root(&root, "yieldspace:plugin/example");
        copy_tree(
            &root.join("wit/deps/yieldspace-plugin/example"),
            &cache_root.join("wit/deps/yieldspace-plugin/example"),
        );
        copy_tree(
            &root.join("wit/deps/test-dep"),
            &cache_root.join("wit/deps/test-dep"),
        );
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache should be written");

        let err = build_project(None, "default", &root)
            .expect_err("build should reject unknown via dependency");
        assert!(err.to_string().contains("via contains unknown dependency"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_legacy_dependency_component_path_field() {
        let root = new_temp_dir("dependency-component-legacy-path");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "wasm"
wit = "file://registry/example.wit"

[dependencies.component]
path = "plugins/example.wasm"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );

        let err = build_project(None, "default", &root)
            .expect_err("legacy component.path must be rejected");
        assert!(
            err.to_string()
                .contains("component.path is no longer supported")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_emits_typed_dependencies_and_capabilities_when_lock_matches() {
        let root = new_temp_dir("dependencies-capabilities-typed");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[capabilities]
privileged = false

[capabilities.deps]
"yieldspace:plugin/example" = ["*"]

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        run_update(&root);

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(manifest.dependencies[0].name, "yieldspace:plugin/example");
        assert!(matches!(
            manifest.dependencies[0].kind,
            ManifestDependencyKind::Native
        ));
        assert_eq!(
            manifest
                .capabilities
                .deps
                .get("yieldspace:plugin/example")
                .cloned(),
            Some(vec!["*".to_string()])
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_transitive_wit_package_digest_mismatch() {
        let root = new_temp_dir("dependencies-transitive-lock-digest-mismatch");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/yieldspace-plugin/example/example.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/test-dep/package.wit"),
            b"package test:dep; interface dep { pong: func() -> string; }\n",
        );

        let digest = compute_path_digest_hex(&root.join("wit/deps/yieldspace-plugin/example"))
            .expect("wit digest should compute");
        let actual_transitive_digest = format!(
            "sha256:{}",
            compute_sha256_hex(&root.join("wit/deps/test-dep/package.wit"))
                .expect("transitive digest should compute")
        );
        write_imago_lock(
            &root,
            &ImagoLock {
                version: IMAGO_LOCK_VERSION,
                dependencies: vec![ImagoLockDependency {
                    name: "yieldspace:plugin/example".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "file://registry/example.wit".to_string(),
                    wit_registry: None,
                    wit_digest: digest.clone(),
                    wit_path: "wit/deps/yieldspace-plugin/example".to_string(),
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                    resolved_at: "0".to_string(),
                }],
                wit_packages: vec![ImagoLockWitPackage {
                    name: "test:dep".to_string(),
                    registry: None,
                    versions: vec![ImagoLockWitPackageVersion {
                        requirement: "*".to_string(),
                        version: None,
                        digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                            .to_string(),
                        source: None,
                        path: "wit/deps/test-dep".to_string(),
                        via: vec!["yieldspace:plugin/example".to_string()],
                    }],
                }],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "yieldspace:plugin/example".to_string(),
            version: "0.1.0".to_string(),
            kind: "native".to_string(),
            wit_source: "file://registry/example.wit".to_string(),
            wit_registry: None,
            wit_path: "wit/deps/yieldspace-plugin/example".to_string(),
            wit_digest: digest,
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            transitive_packages: vec![dependency_cache::DependencyCacheTransitivePackage {
                name: "test:dep".to_string(),
                registry: None,
                requirement: "*".to_string(),
                version: None,
                digest: actual_transitive_digest,
                source: None,
                path: "wit/deps/test-dep".to_string(),
            }],
        };
        let cache_root = dependency_cache::cache_entry_root(&root, "yieldspace:plugin/example");
        copy_tree(
            &root.join("wit/deps/yieldspace-plugin/example"),
            &cache_root.join("wit/deps/yieldspace-plugin/example"),
        );
        copy_tree(
            &root.join("wit/deps/test-dep"),
            &cache_root.join("wit/deps/test-dep"),
        );
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache should be written");

        let err = build_project(None, "default", &root)
            .expect_err("build should reject transitive lock digest mismatch");
        assert!(
            err.to_string()
                .contains("lock digest mismatch for transitive wit package 'test:dep'"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_emits_wasm_component_path_from_lock_hash() {
        let root = new_temp_dir("dependencies-wasm-component-from-lock");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "wasm"
wit = "file://registry/example.wit"

[dependencies.component]
source = "file://registry/example-plugin.wasm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );
        let plugin_bytes = b"\0asmplugin";
        write_file(&root.join("registry/example-plugin.wasm"), plugin_bytes);
        run_update(&root);
        let lock: ImagoLock = toml::from_str(
            &fs::read_to_string(root.join("imago.lock")).expect("lock should exist"),
        )
        .expect("lock should parse");
        let plugin_sha = lock.dependencies[0]
            .component_sha256
            .clone()
            .expect("component sha should be resolved");

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.dependencies.len(), 1);
        let component = manifest.dependencies[0]
            .component
            .as_ref()
            .expect("wasm dependency must include component");
        assert_eq!(component.sha256, plugin_sha);
        assert_eq!(
            component.path,
            format!("plugins/components/{}.wasm", component.sha256)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_emits_wasm_component_path_when_component_is_omitted_in_config() {
        let root = new_temp_dir("dependencies-wasm-component-derived-from-wit");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "chikoski:hello"
version = "0.1.0"
kind = "wasm"
wit = "warg://chikoski:hello@0.1.0"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("wit/deps/chikoski-hello/package.wit"),
            b"package chikoski:hello@0.1.0;\n",
        );
        let wit_digest =
            compute_path_digest_hex(&root.join("wit/deps/chikoski-hello")).expect("wit digest");
        let component_bytes = b"\0asmderived-component";
        let plugin_sha = hex::encode(Sha256::digest(component_bytes));
        write_imago_lock(
            &root,
            &ImagoLock {
                version: IMAGO_LOCK_VERSION,
                dependencies: vec![ImagoLockDependency {
                    name: "chikoski:hello".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "warg://chikoski:hello@0.1.0".to_string(),
                    wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    wit_digest: wit_digest.clone(),
                    wit_path: "wit/deps/chikoski-hello".to_string(),
                    component_source: Some("warg://chikoski:hello@0.1.0".to_string()),
                    component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    component_sha256: Some(plugin_sha.clone()),
                    resolved_at: "0".to_string(),
                }],
                wit_packages: vec![],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "chikoski:hello".to_string(),
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "warg://chikoski:hello@0.1.0".to_string(),
            wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            wit_path: "wit/deps/chikoski-hello".to_string(),
            wit_digest,
            wit_source_fingerprint: None,
            component_source: Some("warg://chikoski:hello@0.1.0".to_string()),
            component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            component_sha256: Some(plugin_sha.clone()),
            component_source_fingerprint: None,
            transitive_packages: vec![],
        };
        let cache_root = dependency_cache::cache_entry_root(&root, "chikoski:hello");
        copy_tree(
            &root.join("wit/deps/chikoski-hello"),
            &cache_root.join("wit/deps/chikoski-hello"),
        );
        write_file(
            &dependency_cache::cache_component_path(&root, "chikoski:hello", &plugin_sha),
            component_bytes,
        );
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache should be written");

        let output = build_project(None, "default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let component = manifest.dependencies[0]
            .component
            .as_ref()
            .expect("wasm dependency must include component");
        assert_eq!(
            component.path,
            format!("plugins/components/{}.wasm", component.sha256)
        );
        assert_eq!(component.sha256, plugin_sha);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_component_source_mismatch_when_component_is_omitted_in_config() {
        let root = new_temp_dir("dependencies-wasm-component-mismatch-derived");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
name = "chikoski:hello"
version = "0.1.0"
kind = "wasm"
wit = "warg://chikoski:hello@0.1.0"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("wit/deps/chikoski-hello/package.wit"),
            b"package chikoski:hello@0.1.0;\n",
        );
        let wit_digest =
            compute_path_digest_hex(&root.join("wit/deps/chikoski-hello")).expect("wit digest");
        let component_bytes = b"\0asmderived-component";
        let plugin_sha = hex::encode(Sha256::digest(component_bytes));
        write_imago_lock(
            &root,
            &ImagoLock {
                version: IMAGO_LOCK_VERSION,
                dependencies: vec![ImagoLockDependency {
                    name: "chikoski:hello".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "warg://chikoski:hello@0.1.0".to_string(),
                    wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    wit_digest: wit_digest.clone(),
                    wit_path: "wit/deps/chikoski-hello".to_string(),
                    component_source: Some("warg://chikoski:other@0.1.0".to_string()),
                    component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    component_sha256: Some(plugin_sha.clone()),
                    resolved_at: "0".to_string(),
                }],
                wit_packages: vec![],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "chikoski:hello".to_string(),
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "warg://chikoski:hello@0.1.0".to_string(),
            wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            wit_path: "wit/deps/chikoski-hello".to_string(),
            wit_digest,
            wit_source_fingerprint: None,
            component_source: Some("warg://chikoski:hello@0.1.0".to_string()),
            component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            component_sha256: Some(plugin_sha.clone()),
            component_source_fingerprint: None,
            transitive_packages: vec![],
        };
        let cache_root = dependency_cache::cache_entry_root(&root, "chikoski:hello");
        copy_tree(
            &root.join("wit/deps/chikoski-hello"),
            &cache_root.join("wit/deps/chikoski-hello"),
        );
        write_file(
            &dependency_cache::cache_component_path(&root, "chikoski:hello", &plugin_sha),
            component_bytes,
        );
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache should be written");

        let err = build_project(None, "default", &root)
            .expect_err("build should fail when derived component source mismatches lock");
        assert!(err.to_string().contains("component source mismatch"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_client_key_path_is_resolved_relative_to_project_root() {
        let root = new_temp_dir("target-client-key-relative");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
client_key = "certs/client.key"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project(None, "default", &root).expect("build should succeed");
        assert_eq!(
            output.target.client_key,
            Some(root.join("certs/client.key"))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_client_key_path_allows_absolute_values() {
        let root = new_temp_dir("target-client-key-absolute");
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
client_key = "{}"
"#,
                abs_client_key.display()
            ),
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project(None, "default", &root).expect("build should succeed");
        assert_eq!(output.target.client_key, Some(abs_client_key));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_client_key_path_rejects_parent_traversal() {
        let root = new_temp_dir("target-client-key-dotdot");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
client_key = "../secrets/client.key"
"#,
        );

        let err = build_project(None, "default", &root)
            .expect_err("target cert path with parent traversal must fail");
        assert!(
            err.to_string()
                .contains("target key 'client_key' must not contain path traversal")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_client_key_path_rejects_backslashes() {
        let root = new_temp_dir("target-client-key-backslash");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
client_key = "certs\\client.key"
"#,
        );

        let err =
            build_project(None, "default", &root).expect_err("backslash path must be rejected");
        assert!(
            err.to_string()
                .contains("target key 'client_key' must not contain backslashes")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_client_key_path_rejects_windows_prefix() {
        let root = new_temp_dir("target-client-key-windows-prefix");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
client_key = "C:/certs/client.key"
"#,
        );

        let err = build_project(None, "default", &root)
            .expect_err("windows-prefixed cert path must be rejected");
        assert!(
            err.to_string()
                .contains("target key 'client_key' must not be windows-prefixed")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_rejects_deprecated_ca_cert_key() {
        let root = new_temp_dir("target-rejects-ca-cert");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
ca_cert = "certs/ca.crt"
"#,
        );

        let err = build_project(None, "default", &root).expect_err("ca_cert should be rejected");
        assert!(err.to_string().contains("ca_cert"));
        assert!(err.to_string().contains("no longer supported"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_rejects_deprecated_client_cert_key() {
        let root = new_temp_dir("target-rejects-client-cert");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
client_cert = "certs/client.crt"
"#,
        );

        let err =
            build_project(None, "default", &root).expect_err("client_cert should be rejected");
        assert!(err.to_string().contains("client_cert"));
        assert!(err.to_string().contains("no longer supported"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn target_rejects_deprecated_known_hosts_key() {
        let root = new_temp_dir("target-rejects-known-hosts");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
known_hosts = "certs/known_hosts"
"#,
        );

        let err =
            build_project(None, "default", &root).expect_err("known_hosts should be rejected");
        assert!(err.to_string().contains("known_hosts"));
        assert!(err.to_string().contains("no longer supported"));

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
            client_key: None,
        };

        let err = target
            .require_deploy_credentials()
            .expect_err("missing cert should fail");
        assert!(err.to_string().contains("client_key"));
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

[env.prod.http]
port = 18080
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

    #[test]
    fn build_defaults_restart_policy_to_never() {
        let root = new_temp_dir("restart-default-never");
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

        let output = build_project(None, "default", &root).expect("build should succeed");
        assert_eq!(output.restart_policy, "never");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_supported_restart_policies() {
        for policy in ["never", "on-failure", "always", "unless-stopped"] {
            let root = new_temp_dir(&format!("restart-policy-{policy}"));
            write_imago_toml(
                &root,
                &format!(
                    r#"
name = "svc"
main = "build/app.wasm"
type = "cli"
restart = "{policy}"

[target.default]
remote = "127.0.0.1:4443"
"#,
                ),
            );
            write_file(&root.join("build/app.wasm"), b"wasm-a");

            let output = build_project(None, "default", &root).expect("build should succeed");
            assert_eq!(output.restart_policy, policy);

            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn build_rejects_invalid_restart_policy() {
        let root = new_temp_dir("restart-policy-invalid");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"
restart = "sometimes"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err =
            build_project(None, "default", &root).expect_err("invalid restart policy should fail");
        assert!(err.to_string().contains("imago.toml key 'restart'"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_legacy_runtime_restart_policy() {
        let root = new_temp_dir("runtime-restart-policy-legacy");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[runtime]
restart_policy = "never"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project(None, "default", &root)
            .expect_err("legacy runtime.restart_policy should fail");
        assert!(err.to_string().contains("runtime.restart_policy"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_service_name_uses_env_override() {
        let root = new_temp_dir("load-service-name-env");
        write_imago_toml(
            &root,
            r#"
name = "svc-default"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"

[env.prod]
name = "svc-prod"
"#,
        );

        let default_name = load_service_name(None, &root).expect("default name should load");
        let env_name = load_service_name(Some("prod"), &root).expect("env name should load");

        assert_eq!(default_name, "svc-default");
        assert_eq!(env_name, "svc-prod");

        let _ = fs::remove_dir_all(root);
    }
}
