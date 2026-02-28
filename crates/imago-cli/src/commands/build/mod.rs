//! Build pipeline for `imago.toml` -> `build/manifest.json`.
//!
//! This module is responsible for:
//! - validating project configuration and mode-specific rules
//! - resolving bindings/dependencies from lock/cache state
//! - materializing a normalized manifest and component artifact
//! - computing integrity metadata consumed by deploy/runtime paths

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Instant,
};

use anyhow::{Context, anyhow};
use dotenvy::from_path_iter;
use imago_lockfile::BindingWitExpectation;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use toml::Value as TomlValue;

use crate::{
    cli::BuildArgs,
    commands::{
        CommandResult, dependency_cache, error_diagnostics, plugin_sources,
        shared::dependency::{DependencyResolver, StandardDependencyResolver},
        ui,
    },
};

mod validation;

const DEFAULT_TARGET_NAME: &str = "default";
const DEFAULT_HTTP_MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HTTP_MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;
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
    assets: Vec<ManifestAsset>,
    #[serde(default)]
    bindings: Vec<ManifestBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http: Option<ManifestHttp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    socket: Option<ManifestSocket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resources: Option<ManifestResourcesConfig>,
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
    name: String,
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
    pub source_kind: plugin_sources::SourceKind,
    pub source: String,
    pub registry: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDependencyComponent {
    pub source_kind: plugin_sources::SourceKind,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectBindingSource {
    pub name: String,
    pub wit_source_kind: plugin_sources::SourceKind,
    pub wit_source: String,
    pub wit_registry: Option<String>,
    pub wit_version: String,
    pub wit_sha256: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ManifestWasiMount {
    asset_dir: String,
    guest_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
struct ManifestResourcesConfig {
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    http_outbound: Vec<String>,
    #[serde(default)]
    mounts: Vec<ManifestWasiMount>,
    #[serde(default)]
    read_only_mounts: Vec<ManifestWasiMount>,
    #[serde(flatten, default)]
    extra: BTreeMap<String, JsonValue>,
}

impl ManifestResourcesConfig {
    fn is_empty(&self) -> bool {
        self.args.is_empty()
            && self.env.is_empty()
            && self.http_outbound.is_empty()
            && self.mounts.is_empty()
            && self.read_only_mounts.is_empty()
            && self.extra.is_empty()
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildCommandLogStream {
    Stdout,
    Stderr,
}

impl BuildCommandLogStream {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildCommandLogLine {
    pub stream: BuildCommandLogStream,
    pub line: String,
}

type BuildCommandLineCallback<'a> = dyn FnMut(&BuildCommandLogLine) + 'a;

#[derive(Debug, Clone)]
pub(crate) struct BuildCommandFailure {
    status_code: Option<i32>,
    logs: Vec<BuildCommandLogLine>,
}

impl BuildCommandFailure {
    fn from_status(status: std::process::ExitStatus, logs: Vec<BuildCommandLogLine>) -> Self {
        Self {
            status_code: status.code(),
            logs,
        }
    }

    #[cfg(test)]
    pub(crate) fn new(status_code: Option<i32>, logs: Vec<BuildCommandLogLine>) -> Self {
        Self { status_code, logs }
    }

    pub(crate) fn logs(&self) -> &[BuildCommandLogLine] {
        &self.logs
    }
}

impl std::fmt::Display for BuildCommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(code) = self.status_code {
            write!(f, "build.command failed with exit code {code}")
        } else {
            write!(f, "build.command was terminated by signal")
        }
    }
}

impl std::error::Error for BuildCommandFailure {}

pub fn run(args: BuildArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(args: BuildArgs, project_root: &Path) -> CommandResult {
    run_with_project_root_and_target_override(args, project_root, None)
}

pub(crate) fn run_with_project_root_and_target_override(
    args: BuildArgs,
    project_root: &Path,
    target_override: Option<&TargetConfig>,
) -> CommandResult {
    let started_at = Instant::now();
    let target_name = args.target.clone();
    ui::command_start("artifact.build", "starting");
    match run_inner_with_target_override(args, project_root, target_override) {
        Ok(output) => {
            ui::command_finish("artifact.build", true, "");
            let mut result = CommandResult::success("artifact.build", started_at);
            result.meta.insert("target".to_string(), target_name);
            result.meta.insert(
                "manifest_path".to_string(),
                output.manifest_path.display().to_string(),
            );
            result
        }
        Err(err) => {
            let summary = error_diagnostics::summarize_command_failure("artifact.build", &err);
            ui::command_finish("artifact.build", false, &summary);
            let message = error_diagnostics::format_command_error("artifact.build", &err);
            CommandResult::failure("artifact.build", started_at, message)
        }
    }
}

fn run_inner_with_target_override(
    args: BuildArgs,
    project_root: &Path,
    target_override: Option<&TargetConfig>,
) -> anyhow::Result<BuildOutput> {
    match target_override {
        Some(target) => {
            build_project_with_target_override(&args.target, project_root, Some(target))
        }
        None => build_project(&args.target, project_root),
    }
}

pub fn load_target_config(target_name: &str, project_root: &Path) -> anyhow::Result<TargetConfig> {
    let root = load_resolved_toml(project_root)?;
    parse_target(&root, target_name, project_root)
}

pub fn load_service_name(project_root: &Path) -> anyhow::Result<String> {
    let root = load_resolved_toml(project_root)?;
    let name = required_string(&root, "name")?;
    validate_service_name(&name)?;
    Ok(name)
}

pub fn build_project(target_name: &str, project_root: &Path) -> anyhow::Result<BuildOutput> {
    build_project_with_target_override(target_name, project_root, None)
}

pub(crate) fn build_project_with_target_override(
    target_name: &str,
    project_root: &Path,
    target_override: Option<&TargetConfig>,
) -> anyhow::Result<BuildOutput> {
    build_project_with_target_override_inner(target_name, project_root, target_override, true, None)
}

pub(crate) fn build_project_with_target_override_for_compose(
    target_name: &str,
    project_root: &Path,
    target_override: Option<&TargetConfig>,
    on_build_line: Option<&mut BuildCommandLineCallback<'_>>,
) -> anyhow::Result<BuildOutput> {
    build_project_with_target_override_inner(
        target_name,
        project_root,
        target_override,
        false,
        on_build_line,
    )
}

pub(crate) fn build_project_with_target_override_for_deploy(
    target_name: &str,
    project_root: &Path,
    target_override: Option<&TargetConfig>,
    on_build_line: &mut BuildCommandLineCallback<'_>,
) -> anyhow::Result<BuildOutput> {
    build_project_with_target_override_inner(
        target_name,
        project_root,
        target_override,
        false,
        Some(on_build_line),
    )
}

fn build_project_with_target_override_inner(
    target_name: &str,
    project_root: &Path,
    target_override: Option<&TargetConfig>,
    emit_progress: bool,
    on_build_line: Option<&mut BuildCommandLineCallback<'_>>,
) -> anyhow::Result<BuildOutput> {
    if emit_progress {
        ui::command_stage("artifact.build", "load-config", "loading imago.toml");
    }
    let root = load_resolved_toml(project_root)?;
    let namespace_registries = parse_namespace_registries(root.get("namespace_registries"))?;

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

    let project_bindings =
        parse_project_binding_sources(root.get("bindings"), Some(&namespace_registries))?;
    let project_dependencies =
        parse_project_dependencies(root.get("dependencies"), Some(&namespace_registries))?;
    if !project_dependencies.is_empty() {
        let wit_deps_root = project_root.join("wit").join("deps");
        if !wit_deps_root.exists() {
            dependency_cache::hydrate_project_wit_deps(
                project_root,
                &project_dependencies,
                Some(&namespace_registries),
            )
            .context("failed to hydrate dependency cache")?;
        } else {
            dependency_cache::verify_project_dependency_cache(
                project_root,
                &project_dependencies,
                Some(&namespace_registries),
            )
            .context("failed to validate dependency cache")?;
        }
    }
    let capabilities = parse_root_capabilities(&root)?;
    let dependency_resolver = StandardDependencyResolver;
    if emit_progress {
        ui::command_stage("artifact.build", "resolve-deps", "resolving dependencies");
    }
    let dependencies = dependency_resolver
        .resolve_manifest_dependencies_from_lock(project_root, &project_dependencies)?;
    let bindings = resolve_manifest_bindings_from_lock(project_root, &project_bindings)?;
    let target = match target_override {
        Some(target) => target.clone(),
        None => parse_target(&root, target_name, project_root)?,
    };

    if emit_progress {
        ui::command_stage(
            "artifact.build",
            "run-build-command",
            "running build command",
        );
    }
    run_build_command(command.as_ref(), project_root, on_build_line)?;

    if emit_progress {
        ui::command_stage(
            "artifact.build",
            "materialize",
            "materializing hashed artifact",
        );
    }
    ensure_file_exists(project_root, &source_main_path, "main")?;
    let materialized_main_path = materialize_hashed_wasm(project_root, &source_main_path, &name)?;
    let manifest_main = materialized_main_path
        .file_name()
        .ok_or_else(|| anyhow!("materialized wasm filename is missing"))?
        .to_os_string();

    let assets = parse_assets(root.get("assets"), project_root)?;
    let mut resources = parse_resources_section(&root, &assets)?;
    let dotenv_resources_env = load_dotenv_resources_env(project_root)?;
    if !dotenv_resources_env.is_empty() {
        let resolved = resources.get_or_insert_with(ManifestResourcesConfig::default);
        for (key, value) in dotenv_resources_env {
            resolved.env.insert(key, value);
        }
    }

    let mut manifest = Manifest {
        name,
        main: normalized_path_to_string(Path::new(&manifest_main)),
        app_type,
        target: target.as_manifest_target_map(),
        assets: assets
            .iter()
            .map(|entry| entry.manifest_asset.clone())
            .collect(),
        bindings,
        http,
        socket,
        resources,
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

    if emit_progress {
        ui::command_stage(
            "artifact.build",
            "write-manifest",
            "writing build/manifest.json",
        );
    }
    let manifest_path = resolve_manifest_output_path();
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

fn load_resolved_toml(project_root: &Path) -> anyhow::Result<toml::Table> {
    let path = project_root.join("imago.toml");
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: TomlValue = toml::from_str(&raw).context("failed to parse imago.toml")?;
    let root = parsed
        .as_table()
        .cloned()
        .ok_or_else(|| anyhow!("imago.toml root must be a table"))?;

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

pub(crate) fn is_supported_restart_policy(value: &str) -> bool {
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
    on_build_line: Option<&mut BuildCommandLineCallback<'_>>,
) -> anyhow::Result<()> {
    let Some(command) = command else {
        return Ok(());
    };

    if let Some(on_build_line) = on_build_line {
        return run_build_command_capture_for_deploy(command, project_root, on_build_line);
    }
    run_build_command_passthrough(command, project_root)
}

fn run_build_command_passthrough(
    command: &BuildCommand,
    project_root: &Path,
) -> anyhow::Result<()> {
    let mut process = make_build_process(command, project_root);
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

fn run_build_command_capture_for_deploy(
    command: &BuildCommand,
    project_root: &Path,
    on_build_line: &mut BuildCommandLineCallback<'_>,
) -> anyhow::Result<()> {
    let mut process = make_build_process(command, project_root);
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());
    let mut child = process.spawn().context("failed to run build.command")?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture build.command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture build.command stderr"))?;

    let (tx, rx) = mpsc::channel::<anyhow::Result<BuildCommandLogLine>>();
    let stdout_handle =
        spawn_build_command_reader(stdout, BuildCommandLogStream::Stdout, tx.clone());
    let stderr_handle = spawn_build_command_reader(stderr, BuildCommandLogStream::Stderr, tx);

    let mut logs = Vec::new();
    let mut read_error: Option<anyhow::Error> = None;
    for event in rx {
        match event {
            Ok(log) => {
                on_build_line(&log);
                logs.push(log);
            }
            Err(err) => {
                if read_error.is_none() {
                    read_error = Some(err);
                }
            }
        }
    }

    let status = child.wait().context("failed to wait for build.command")?;

    if stdout_handle.join().is_err() {
        return Err(anyhow!("build.command stdout reader thread panicked"));
    }
    if stderr_handle.join().is_err() {
        return Err(anyhow!("build.command stderr reader thread panicked"));
    }

    if let Some(err) = read_error {
        return Err(err);
    }

    if status.success() {
        return Ok(());
    }

    Err(BuildCommandFailure::from_status(status, logs).into())
}

fn spawn_build_command_reader<R: Read + Send + 'static>(
    reader: R,
    stream: BuildCommandLogStream,
    sender: mpsc::Sender<anyhow::Result<BuildCommandLogLine>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    let line = decode_build_command_log_line(&buf);
                    if sender
                        .send(Ok(BuildCommandLogLine { stream, line }))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(err) => {
                    let _ = sender.send(Err(anyhow!(
                        "failed to read build.command {}: {err}",
                        stream.as_str()
                    )));
                    break;
                }
            }
        }
    })
}

fn decode_build_command_log_line(raw_line: &[u8]) -> String {
    String::from_utf8_lossy(raw_line)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn make_build_process(command: &BuildCommand, project_root: &Path) -> Command {
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
    process
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

pub(crate) fn validate_app_type(app_type: &str) -> anyhow::Result<()> {
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
    let root = load_resolved_toml(project_root)?;
    parse_namespace_registries(root.get("namespace_registries"))
}

pub(crate) fn load_project_dependencies_with_namespace_registries(
    project_root: &Path,
    namespace_registries: &plugin_sources::NamespaceRegistries,
) -> anyhow::Result<Vec<ProjectDependency>> {
    let root = load_resolved_toml(project_root)?;
    parse_project_dependencies(root.get("dependencies"), Some(namespace_registries))
}

pub(crate) fn load_project_binding_sources_with_namespace_registries(
    project_root: &Path,
    namespace_registries: &plugin_sources::NamespaceRegistries,
) -> anyhow::Result<Vec<ProjectBindingSource>> {
    let root = load_resolved_toml(project_root)?;
    parse_project_binding_sources(root.get("bindings"), Some(namespace_registries))
}

fn parse_project_dependencies(
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

fn parse_dependency_wit_source(
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

fn dependency_name_hint_from_source(
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

fn parse_source_selector(
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

    let deps = parse_deps_capability_rules(table.get("deps"), &format!("{field_name}.deps"))?;
    let wasi = parse_wasi_capability_rules(table.get("wasi"), &format!("{field_name}.wasi"))?;

    Ok(ManifestCapabilityPolicy {
        privileged,
        deps,
        wasi,
    })
}

fn parse_deps_capability_rules(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };

    if let Some(rule) = value.as_str() {
        if rule.trim() == "*" {
            return Ok(BTreeMap::from([("*".to_string(), vec!["*".to_string()])]));
        }
        return Err(anyhow!("{field_name} must be \"*\" or a table"));
    }

    if value.as_table().is_none() {
        return Err(anyhow!("{field_name} must be \"*\" or a table"));
    }

    parse_capability_rule_table(Some(value), field_name)
}

fn parse_wasi_capability_rules(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };

    if let Some(allow_all) = value.as_bool() {
        if allow_all {
            return Ok(BTreeMap::from([("*".to_string(), vec!["*".to_string()])]));
        }
        return Ok(BTreeMap::new());
    }

    parse_capability_rule_table(Some(value), field_name)
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

fn parse_project_binding_sources(
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

fn resolve_manifest_bindings_from_lock(
    project_root: &Path,
    bindings: &[ProjectBindingSource],
) -> anyhow::Result<Vec<ManifestBinding>> {
    if bindings.is_empty() {
        return Ok(Vec::new());
    }

    let lock = imago_lockfile::load_from_project_root(project_root)?;
    let mut expectations = Vec::with_capacity(bindings.len());
    for binding in bindings {
        expectations.push(BindingWitExpectation {
            name: binding.name.clone(),
            wit_source: binding.wit_source.clone(),
            wit_registry: binding.wit_registry.clone(),
            wit_version: binding.wit_version.clone(),
        });
    }
    let resolved = imago_lockfile::resolve_binding_wits(project_root, &lock, &expectations)?;

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
    Ok(Some(resolve_target_credential_path(
        text,
        key,
        project_root,
    )?))
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

pub(crate) fn resolve_target_credential_path(
    raw: &str,
    key: &str,
    project_root: &Path,
) -> anyhow::Result<PathBuf> {
    let normalized = normalize_target_credential_path(raw, key)?;
    if normalized.is_absolute() {
        Ok(normalized)
    } else {
        Ok(project_root.join(normalized))
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

fn parse_resources_section(
    root: &toml::Table,
    assets: &[AssetSource],
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
        extra.insert(key.clone(), toml_to_json_normalized(value)?);
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

fn parse_resources_args(value: Option<&TomlValue>) -> anyhow::Result<Vec<String>> {
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

fn parse_resources_http_outbound(value: Option<&TomlValue>) -> anyhow::Result<Vec<String>> {
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

fn normalize_wasi_http_outbound_rule(raw: &str, field_name: &str) -> anyhow::Result<String> {
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

fn normalize_wasi_http_outbound_cidr(value: &str, field_name: &str) -> anyhow::Result<String> {
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

fn normalize_wasi_http_outbound_host_or_host_port(
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

fn normalize_wasi_http_outbound_host(raw_host: &str, field_name: &str) -> anyhow::Result<String> {
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

fn parse_wasi_http_outbound_port(port_text: &str, field_name: &str) -> anyhow::Result<u16> {
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

fn cidr_network_ip(ip: IpAddr, prefix: u8) -> IpAddr {
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

fn load_dotenv_resources_env(project_root: &Path) -> anyhow::Result<BTreeMap<String, String>> {
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

fn collect_allowed_resource_asset_dirs(assets: &[AssetSource]) -> BTreeSet<PathBuf> {
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

fn parse_resource_mount_entries(
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

fn validate_resource_mount_uniqueness(
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

fn normalize_wasi_guest_path(raw: &str, field_name: &str) -> anyhow::Result<String> {
    let path = Path::new(raw.trim());
    if path.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }
    if raw.contains('\\') {
        return Err(anyhow!(
            "{field_name} must not contain backslashes: {}",
            raw.trim()
        ));
    }
    if !path.is_absolute() {
        return Err(anyhow!(
            "{field_name} must be an absolute path: {}",
            raw.trim()
        ));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(anyhow!(
            "{field_name} must not be windows-prefixed: {}",
            raw.trim()
        ));
    }

    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(segment) => {
                segments.push(segment.to_string_lossy().to_string());
            }
            Component::ParentDir | Component::CurDir => {
                return Err(anyhow!(
                    "{field_name} must not contain path traversal: {}",
                    raw.trim()
                ));
            }
            _ => {
                return Err(anyhow!("{field_name} is invalid: {}", raw.trim()));
            }
        }
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
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

pub fn resolve_manifest_output_path() -> PathBuf {
    PathBuf::from("build/manifest.json")
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
        IMAGO_LOCK_VERSION, ImagoLock, ImagoLockBindingWit, ImagoLockDependency,
        ImagoLockWitPackage, ImagoLockWitPackageVersion,
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

    fn read_manifest_json(root: &Path, relative_path: &Path) -> serde_json::Value {
        let bytes = fs::read(root.join(relative_path)).expect("manifest should exist");
        serde_json::from_slice(&bytes).expect("manifest json value should parse")
    }

    fn run_update(root: &Path) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime for update helper should be created");
        let result = runtime.block_on(update::run_with_project_root(UpdateArgs {}, root));
        assert_eq!(
            result.exit_code, 0,
            "imago deps sync should succeed before build tests: {:?}",
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
    fn build_generates_default_manifest_and_ignores_legacy_vars_and_secrets() {
        let root = new_temp_dir("default-manifest-legacy-env");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "http"

[http]
port = 18080

[vars]
VISIBLE = "1"

[secrets]
SECRET_TOKEN = "abc"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        assert_eq!(output.manifest_path, PathBuf::from("build/manifest.json"));
        assert!(root.join("build/manifest.json").exists());

        let manifest = read_manifest(&root, &output.manifest_path);
        let manifest_json = read_manifest_json(&root, &output.manifest_path);
        let object = manifest_json
            .as_object()
            .expect("manifest json root should be object");
        assert_eq!(manifest.app_type, "http");
        assert_eq!(manifest.http.as_ref().map(|v| v.port), Some(18080));
        assert_eq!(
            manifest.http.as_ref().map(|v| v.max_body_bytes),
            Some(DEFAULT_HTTP_MAX_BODY_BYTES)
        );
        assert!(!object.contains_key("vars"));
        assert!(!object.contains_key("secrets"));
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

        let output = build_project("default", &root).expect("build should succeed");
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

        let output = build_project("default", &root).expect("build should succeed");
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

        let err = build_project("default", &root)
            .expect_err("build must fail when type=http has no http.port");
        assert!(err.to_string().contains("requires [http] table"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_http_section_for_non_http_type() {
        for app_type in ["cli", "socket", "rpc"] {
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

            let err = build_project("default", &root)
                .expect_err("build must fail when non-http type uses [http]");
            assert!(
                err.to_string()
                    .contains("http section is only allowed when type is \"http\"")
            );

            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn build_succeeds_for_rpc_type_without_http_or_socket() {
        let root = new_temp_dir("rpc-type-valid");
        write_imago_toml(
            &root,
            r#"
name = "svc-rpc"
main = "build/app.wasm"
type = "rpc"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-rpc");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.app_type, "rpc");
        assert!(manifest.http.is_none());
        assert!(manifest.socket.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_invalid_http_max_body_bytes() {
        for (suffix, max_body_bytes) in [("zero", "0"), ("too-large", "33554433")] {
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

            let err =
                build_project("default", &root).expect_err("invalid max_body_bytes must fail");
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

        let output = build_project("default", &root).expect("build should succeed");
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

        let err =
            build_project("default", &root).expect_err("type=socket without [socket] must fail");
        assert!(
            err.to_string()
                .contains("type=\"socket\" requires [socket] table")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_socket_section_for_non_socket_type() {
        for (app_type, app_type_extra) in
            [("cli", ""), ("http", "[http]\nport = 18080\n"), ("rpc", "")]
        {
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

            let err = build_project("default", &root)
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

            let err = build_project("default", &root).expect_err("invalid socket section");
            assert!(
                err.to_string().contains(expected),
                "unexpected error for {suffix}: {err}"
            );

            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn build_does_not_merge_env_table_overrides() {
        let root = new_temp_dir("env-table-ignored");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[vars]
A = "1"

[target.default]
remote = "127.0.0.1:4443"

[env.prod]
type = "http"

[env.prod.vars]
C = "3"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let manifest_json = read_manifest_json(&root, &output.manifest_path);
        let object = manifest_json
            .as_object()
            .expect("manifest json root should be object");
        assert_eq!(manifest.app_type, "cli");
        assert!(!object.contains_key("vars"));
        assert!(!object.contains_key("secrets"));

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

        let output = build_project("default", &root).expect("build should succeed");
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

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        assert_eq!(
            fs::read(root.join("build/app.wasm")).unwrap(),
            fs::read(root.join(hashed_main)).unwrap()
        );

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

        let err = build_project("default", &root).expect_err("missing main file should fail");
        assert!(err.to_string().contains("main file is not accessible"));

        write_file(&root.join("build/app.wasm"), b"wasm-a");
        let output = build_project("default", &root).expect("build should succeed");
        assert_eq!(output.manifest_path, PathBuf::from("build/manifest.json"));
        let manifest = read_manifest(&root, &output.manifest_path);
        let hashed_main = assert_hashed_main_path(&manifest, "svc");
        assert!(root.join(hashed_main).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capture_for_deploy_streams_build_command_lines() {
        let root = new_temp_dir("capture-for-deploy-lines");
        let command = BuildCommand::Argv(vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf 'out-a\\n'; printf 'err-a\\n' >&2".to_string(),
        ]);
        let mut captured = Vec::new();
        let mut on_line = |line: &BuildCommandLogLine| captured.push(line.clone());

        run_build_command(Some(&command), &root, Some(&mut on_line))
            .expect("capture mode build.command should succeed");

        assert!(
            captured.iter().any(|line| {
                line.stream == BuildCommandLogStream::Stdout && line.line == "out-a"
            })
        );
        assert!(
            captured.iter().any(|line| {
                line.stream == BuildCommandLogStream::Stderr && line.line == "err-a"
            })
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compose_build_streams_build_command_lines_when_callback_is_set() {
        let root = new_temp_dir("compose-capture-lines");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[build]
command = ["sh", "-c", "mkdir -p build && printf 'compose-out\n' && printf 'compose-err\n' >&2 && printf wasm > build/app.wasm"]

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let mut streamed = Vec::new();
        let mut on_line = |line: &BuildCommandLogLine| streamed.push(line.clone());

        let output = build_project_with_target_override_for_compose(
            "default",
            &root,
            None,
            Some(&mut on_line),
        )
        .expect("compose build with callback should succeed");
        assert_eq!(output.manifest_path, PathBuf::from("build/manifest.json"));
        assert!(streamed.iter().any(|line| {
            line.stream == BuildCommandLogStream::Stdout && line.line == "compose-out"
        }));
        assert!(streamed.iter().any(|line| {
            line.stream == BuildCommandLogStream::Stderr && line.line == "compose-err"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capture_for_deploy_failure_keeps_full_logs() {
        let root = new_temp_dir("capture-for-deploy-failure");
        let command = BuildCommand::Argv(vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf 'ok-before-fail\\n'; printf 'err-before-fail\\n' >&2; exit 7".to_string(),
        ]);
        let mut streamed = Vec::new();
        let mut on_line = |line: &BuildCommandLogLine| streamed.push(line.clone());

        let err = run_build_command(Some(&command), &root, Some(&mut on_line))
            .expect_err("capture mode build.command should fail");
        let failure = err
            .downcast_ref::<BuildCommandFailure>()
            .expect("error should be BuildCommandFailure");

        assert!(err.to_string().contains("exit code 7"));
        assert!(streamed.iter().any(|line| {
            line.stream == BuildCommandLogStream::Stdout && line.line == "ok-before-fail"
        }));
        assert!(streamed.iter().any(|line| {
            line.stream == BuildCommandLogStream::Stderr && line.line == "err-before-fail"
        }));
        assert!(failure.logs().iter().any(|line| {
            line.stream == BuildCommandLogStream::Stdout && line.line == "ok-before-fail"
        }));
        assert!(failure.logs().iter().any(|line| {
            line.stream == BuildCommandLogStream::Stderr && line.line == "err-before-fail"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_command_reader_handles_non_utf8_bytes() {
        let reader = std::io::Cursor::new(vec![b'o', b'k', 0xff, b'\n', 0xfe]);
        let (sender, receiver) = mpsc::channel();
        let handle = spawn_build_command_reader(reader, BuildCommandLogStream::Stdout, sender);

        let events = receiver
            .into_iter()
            .map(|event| event.expect("reader should not emit errors"))
            .collect::<Vec<_>>();

        handle
            .join()
            .expect("build.command reader thread should not panic");

        assert_eq!(
            events,
            vec![
                BuildCommandLogLine {
                    stream: BuildCommandLogStream::Stdout,
                    line: "ok\u{fffd}".to_string(),
                },
                BuildCommandLogLine {
                    stream: BuildCommandLogStream::Stdout,
                    line: "\u{fffd}".to_string(),
                },
            ]
        );
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

        let err = build_project("default", &root)
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

        let output = build_project("default", &root).expect("build should succeed");
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
    fn manifest_expands_bindings_from_lock_sources() {
        let root = new_temp_dir("manifest-bindings");
        write_imago_toml(
            &root,
            r#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"

[[bindings]]
name = "svc-b"
version = "0.1.0"
path = "registry/acme-clock"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(
            &root.join("registry/acme-clock/package.wit"),
            b"package acme:clock@0.1.0;\ninterface api { now: func() -> u64; }\n",
        );
        write_file(
            &root.join("wit/deps/acme-clock/package.wit"),
            br#"
package acme:clock@0.1.0;

interface api {
  now: func() -> u64;
}

interface admin {
  health: func() -> string;
}
"#,
        );
        let wit_digest =
            compute_path_digest_hex(&root.join("wit/deps/acme-clock")).expect("wit digest");
        write_imago_lock(
            &root,
            &ImagoLock {
                version: IMAGO_LOCK_VERSION,
                dependencies: vec![],
                wit_packages: vec![],
                binding_wits: vec![ImagoLockBindingWit {
                    name: "svc-b".to_string(),
                    wit_source: "registry/acme-clock".to_string(),
                    wit_registry: None,
                    wit_version: "0.1.0".to_string(),
                    wit_digest,
                    wit_path: "wit/deps/acme-clock".to_string(),
                    interfaces: vec!["acme:clock/admin".to_string(), "acme:clock/api".to_string()],
                }],
            },
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.bindings.len(), 2);
        let actual = manifest
            .bindings
            .iter()
            .map(|binding| (binding.name.clone(), binding.wit.clone()))
            .collect::<BTreeSet<_>>();
        let expected = [
            ("svc-b".to_string(), "acme:clock/admin".to_string()),
            ("svc-b".to_string(), "acme:clock/api".to_string()),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);

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
name = "svc-b"
version = "0.1.0"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root)
            .expect_err("build must fail when bindings.wit is missing");
        assert!(
            err.to_string()
                .contains("bindings[0] must define exactly one source key")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_bindings_target_key() {
        let root = new_temp_dir("manifest-bindings-target-key");
        write_imago_toml(
            &root,
            r#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"

[[bindings]]
name = "svc-b"
version = "0.1.0"
path = "registry/acme-clock"
target = "legacy"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root).expect_err("build must fail on bindings.target");
        assert!(err.to_string().contains("bindings[0].target"));
        assert!(err.to_string().contains("no longer supported"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_legacy_bindings_wit_format() {
        let root = new_temp_dir("manifest-bindings-legacy-wit-format");
        write_imago_toml(
            &root,
            r#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"

[[bindings]]
name = "svc-b"
version = "0.1.0"
wit = "yieldspace:svc/invoke"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err =
            build_project("default", &root).expect_err("legacy bindings format must be rejected");
        assert!(err.to_string().contains("imago.lock is missing"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_bindings_wit_without_supported_scheme() {
        let root = new_temp_dir("manifest-bindings-invalid-wit-scheme");
        write_imago_toml(
            &root,
            r#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"

[[bindings]]
name = "svc-b"
version = "0.1.0"
wit = "https://example.invalid/acme-clock.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root)
            .expect_err("build must fail when bindings.wit scheme is unsupported");
        assert!(
            err.to_string()
                .contains("must not use URL scheme; use plain package name")
        );

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

        let err = build_project("default", &root).expect_err("typo key must be rejected");
        assert!(err.to_string().contains("capabilirties"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_capabilities_deps_wildcard_string() {
        let root = new_temp_dir("capability-deps-wildcard");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[capabilities]
deps = "*"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(
            manifest.capabilities.deps.get("*").cloned(),
            Some(vec!["*".to_string()])
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_capabilities_deps_non_wildcard_string() {
        let root = new_temp_dir("capability-deps-non-wildcard-string");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[capabilities]
deps = "invoke"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root)
            .expect_err("capabilities.deps must reject non-wildcard string");
        assert!(
            err.to_string()
                .contains("capabilities.deps must be \"*\" or a table"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_capabilities_wasi_true_as_wildcard() {
        let root = new_temp_dir("capability-wasi-bool-true");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[capabilities]
wasi = true

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(
            manifest.capabilities.wasi.get("*").cloned(),
            Some(vec!["*".to_string()])
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_capabilities_wasi_false_as_empty_policy() {
        let root = new_temp_dir("capability-wasi-bool-false");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[capabilities]
wasi = false

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert!(manifest.capabilities.wasi.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_wasi_section_with_mounts_and_read_only_mounts() {
        let root = new_temp_dir("wasi-section-mounts");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[assets]]
path = "assets/rw/input.txt"

[[assets]]
path = "assets/ro/input.txt"

[resources]
args = ["--serve"]

[resources.env]
WASI_ONLY = "1"

[[resources.mounts]]
asset_dir = "assets/rw"
guest_path = "/guest/rw"

[[resources.read_only_mounts]]
asset_dir = "assets/ro"
guest_path = "/guest/ro"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join("assets/rw/input.txt"), b"rw");
        write_file(&root.join("assets/ro/input.txt"), b"ro");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let resources = manifest
            .resources
            .expect("resources section should be emitted");
        assert_eq!(resources.args, vec!["--serve".to_string()]);
        assert_eq!(resources.env.get("WASI_ONLY"), Some(&"1".to_string()));
        assert!(resources.http_outbound.is_empty());
        assert_eq!(
            resources.mounts,
            vec![ManifestWasiMount {
                asset_dir: "assets/rw".to_string(),
                guest_path: "/guest/rw".to_string(),
            }]
        );
        assert_eq!(
            resources.read_only_mounts,
            vec![ManifestWasiMount {
                asset_dir: "assets/ro".to_string(),
                guest_path: "/guest/ro".to_string(),
            }]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_resources_custom_fields() {
        let root = new_temp_dir("resources-custom-fields");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[resources]
feature_enabled = true
allowed_devices = ["/dev/i2c-1", "/dev/i2c-2"]

[resources.policy]
mode = "strict"
retry = 3

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let resources = manifest
            .resources
            .expect("resources section should be emitted");
        assert_eq!(
            resources.extra.get("feature_enabled"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            resources.extra.get("allowed_devices"),
            Some(&serde_json::json!(["/dev/i2c-1", "/dev/i2c-2"]))
        );
        assert_eq!(
            resources.extra.get("policy"),
            Some(&serde_json::json!({ "mode": "strict", "retry": 3 }))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_wasi_http_outbound_and_normalizes_entries() {
        let root = new_temp_dir("wasi-http-outbound");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[resources]
http_outbound = [
  "LOCALHOST",
  "api.example.com:443",
  "10.1.2.3/8",
  "api.example.com:443",
  "127.0.0.1"
]

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let resources = manifest.resources.expect("resources section should exist");
        assert_eq!(
            resources.http_outbound,
            vec![
                "localhost".to_string(),
                "api.example.com:443".to_string(),
                "10.0.0.0/8".to_string(),
                "127.0.0.1".to_string(),
            ]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_wasi_http_outbound_non_array() {
        let root = new_temp_dir("wasi-http-outbound-non-array");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[resources]
http_outbound = "localhost"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root).expect_err("build should fail");
        assert!(
            err.to_string()
                .contains("resources.http_outbound must be an array")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_wasi_http_outbound_wildcard() {
        let root = new_temp_dir("wasi-http-outbound-wildcard");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[resources]
http_outbound = ["*.example.com"]

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root).expect_err("build should fail");
        assert!(err.to_string().contains("wildcard is not supported"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_wasi_http_outbound_invalid_cidr() {
        let root = new_temp_dir("wasi-http-outbound-cidr-invalid");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[resources]
http_outbound = ["10.0.0.0/99"]

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root).expect_err("build should fail");
        assert!(err.to_string().contains("CIDR prefix must be in range"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_ignores_legacy_vars_and_keeps_wasi_env() {
        let root = new_temp_dir("wasi-env-duplicate");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[vars]
SHARED = "a"

[resources.env]
SHARED = "b"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let resources = manifest.resources.expect("resources section should exist");
        assert_eq!(resources.env.get("SHARED"), Some(&"b".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_merges_dotenv_into_wasi_env_with_dotenv_precedence() {
        let root = new_temp_dir("dotenv-merge-wasi-env");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[resources.env]
FROM_TOML = "toml"
SHARED = "toml"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join(".env"),
            b"SHARED=dotenv\nFROM_DOTENV=dotenv-value\nQUOTED=\"hello world\"\n",
        );

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let resources = manifest.resources.expect("resources section should exist");

        assert_eq!(resources.env.get("FROM_TOML"), Some(&"toml".to_string()));
        assert_eq!(resources.env.get("SHARED"), Some(&"dotenv".to_string()));
        assert_eq!(
            resources.env.get("FROM_DOTENV"),
            Some(&"dotenv-value".to_string())
        );
        assert_eq!(
            resources.env.get("QUOTED"),
            Some(&"hello world".to_string())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_creates_wasi_env_from_dotenv_without_wasi_section() {
        let root = new_temp_dir("dotenv-only-wasi-env");
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
        write_file(&root.join(".env"), b"DOTENV_ONLY=1\n");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let resources = manifest.resources.expect("resources section should exist");

        assert_eq!(resources.env.get("DOTENV_ONLY"), Some(&"1".to_string()));
        assert!(resources.args.is_empty());
        assert!(resources.mounts.is_empty());
        assert!(resources.read_only_mounts.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_malformed_dotenv() {
        let root = new_temp_dir("dotenv-invalid");
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
        write_file(&root.join(".env"), b"INVALID_LINE\n");

        let err = build_project("default", &root).expect_err("build should fail");
        assert!(err.to_string().contains("failed to parse"));
        assert!(err.to_string().contains(".env"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_succeeds_when_dotenv_is_missing() {
        let root = new_temp_dir("dotenv-missing");
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

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert!(manifest.resources.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_wasi_mount_duplicate_guest_path_across_sections() {
        let root = new_temp_dir("wasi-mount-duplicate-guest");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[assets]]
path = "assets/rw/input.txt"

[[assets]]
path = "assets/ro/input.txt"

[[resources.mounts]]
asset_dir = "assets/rw"
guest_path = "/guest/shared"

[[resources.read_only_mounts]]
asset_dir = "assets/ro"
guest_path = "/guest/shared"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join("assets/rw/input.txt"), b"rw");
        write_file(&root.join("assets/ro/input.txt"), b"ro");

        let err = build_project("default", &root).expect_err("duplicate guest path must fail");
        assert!(err.to_string().contains("duplicate guest_path"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_wasi_mount_duplicate_asset_dir_across_sections() {
        let root = new_temp_dir("wasi-mount-duplicate-asset-dir");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[assets]]
path = "assets/shared/input.txt"

[[resources.mounts]]
asset_dir = "assets/shared"
guest_path = "/guest/rw"

[[resources.read_only_mounts]]
asset_dir = "assets/shared"
guest_path = "/guest/ro"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join("assets/shared/input.txt"), b"shared");

        let err = build_project("default", &root).expect_err("duplicate asset_dir must fail");
        assert!(err.to_string().contains("duplicate asset_dir"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_wasi_read_only_mounts_without_rw_mounts() {
        let root = new_temp_dir("wasi-read-only-only");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[assets]]
path = "assets/ro/input.txt"

[[resources.read_only_mounts]]
asset_dir = "assets/ro"
guest_path = "/guest/ro"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join("assets/ro/input.txt"), b"ro");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        let resources = manifest
            .resources
            .expect("resources section should be emitted");
        assert!(resources.mounts.is_empty());
        assert_eq!(
            resources.read_only_mounts,
            vec![ManifestWasiMount {
                asset_dir: "assets/ro".to_string(),
                guest_path: "/guest/ro".to_string(),
            }]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_rejects_wasi_mount_asset_dir_not_derived_from_assets() {
        let root = new_temp_dir("wasi-mount-asset-dir-invalid");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[assets]]
path = "assets/rw/input.txt"

[[resources.mounts]]
asset_dir = "assets/not-listed"
guest_path = "/guest/rw"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(&root.join("assets/rw/input.txt"), b"rw");

        let err = build_project("default", &root)
            .expect_err("mount source outside assets dirs must fail");
        assert!(
            err.to_string()
                .contains("must match a directory derived from assets[].path")
        );

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
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        fs::remove_file(root.join("imago.lock")).expect("lock should be removable");

        let err =
            build_project("default", &root).expect_err("build should fail when lock is missing");
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
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        fs::remove_dir_all(root.join("wit/deps")).expect("wit/deps should be removable");

        build_project("default", &root)
            .expect("build should succeed by hydrating wit/deps from dependency cache");
        assert!(
            root.join("wit/deps/test-example/package.wit")
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
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        fs::remove_dir_all(root.join(".imago/deps")).expect("dependency cache should be removable");

        let err = build_project("default", &root)
            .expect_err("build should fail when dependency cache is missing");
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains(".imago/deps"),
            "unexpected error: {err:#}"
        );
        assert!(
            err_chain.contains("run `imago deps sync`"),
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
version = "0.1.0"
kind = "native"
wit = "/tmp/pwn"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root).expect_err("absolute dependency source must fail");
        let err_text = format!("{err:#}");
        assert!(
            err_text.contains("failed to parse dependencies[0] source configuration"),
            "unexpected error: {err_text}"
        );
        assert!(
            err_text.contains("warg source package contains invalid path components: /tmp/pwn"),
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
version = "0.1.0"
kind = "native"
wit = "yieldspace:plugin/../example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err = build_project("default", &root)
            .expect_err("dependency package with normalized path segment must fail");
        let err_text = format!("{err:#}");
        assert!(
            err_text.contains("failed to parse dependencies[0] source configuration"),
            "unexpected error: {err_text}"
        );
        assert!(
            err_text.contains("warg source package contains invalid path components"),
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
version = "0.1.0"
kind = "native"
path = "registry/example"
requires = ["/tmp/pwn"]

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let err =
            build_project("default", &root).expect_err("absolute dependency requirement must fail");
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
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        run_update(&root);
        let mut lock: ImagoLock = toml::from_str(
            &fs::read_to_string(root.join("imago.lock")).expect("lock should exist"),
        )
        .expect("lock should parse");
        lock.version = 2;
        write_imago_lock(&root, &lock);

        let err = build_project("default", &root)
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
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/path-source-0/package.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/test-dep/package.wit"),
            b"package test:dep; interface dep { pong: func() -> string; }\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/path-source-0"))
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
                    name: "path-source-0".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "registry/example".to_string(),
                    wit_registry: None,
                    wit_digest: digest.clone(),
                    wit_path: "wit/deps/path-source-0".to_string(),
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                }],
                binding_wits: vec![],
                wit_packages: vec![ImagoLockWitPackage {
                    name: "test:dep".to_string(),
                    registry: None,
                    versions: vec![ImagoLockWitPackageVersion {
                        requirement: "*".to_string(),
                        version: None,
                        digest: transitive_digest.clone(),
                        source: None,
                        path: "wit/deps/test-dep".to_string(),
                        via: vec!["path-source-1".to_string()],
                    }],
                }],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "path-source-0".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "native".to_string(),
            wit_source: "registry/example".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: "wit/deps/path-source-0".to_string(),
            wit_digest: digest,
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
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
        let cache_root = dependency_cache::cache_entry_root(&root, "path-source-0");
        copy_tree(
            &root.join("wit/deps/path-source-0"),
            &cache_root.join("wit/deps/path-source-0"),
        );
        copy_tree(
            &root.join("wit/deps/test-dep"),
            &cache_root.join("wit/deps/test-dep"),
        );
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache should be written");

        let err = build_project("default", &root)
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
version = "0.1.0"
kind = "wasm"
path = "registry/example"

[dependencies.component]
source = "plugins/example.wasm"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );

        let err =
            build_project("default", &root).expect_err("legacy component.path must be rejected");
        assert!(
            err.to_string()
                .contains("dependencies[0].component.source is not supported")
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
"test:example" = ["*"]

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        run_update(&root);

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(manifest.dependencies[0].name, "test:example");
        assert!(matches!(
            manifest.dependencies[0].kind,
            ManifestDependencyKind::Native
        ));
        assert_eq!(
            manifest
                .capabilities
                .deps
                .get("test:example")
                .cloned(),
            Some(vec!["*".to_string()])
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_accepts_dependency_capabilities_deps_wildcard_string() {
        let root = new_temp_dir("dependencies-capabilities-deps-wildcard");
        write_imago_toml(
            &root,
            r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/example"

[dependencies.capabilities]
deps = "*"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        run_update(&root);

        let output = build_project("default", &root).expect("build should succeed");
        let manifest = read_manifest(&root, &output.manifest_path);
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(
            manifest.dependencies[0].capabilities.deps.get("*").cloned(),
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
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/path-source-0/package.wit"),
            b"package test:example;\n",
        );
        write_file(
            &root.join("wit/deps/test-dep/package.wit"),
            b"package test:dep; interface dep { pong: func() -> string; }\n",
        );

        let digest = compute_path_digest_hex(&root.join("wit/deps/path-source-0"))
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
                    name: "path-source-0".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "registry/example".to_string(),
                    wit_registry: None,
                    wit_digest: digest.clone(),
                    wit_path: "wit/deps/path-source-0".to_string(),
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                }],
                binding_wits: vec![],
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
                        via: vec!["path-source-0".to_string()],
                    }],
                }],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "path-source-0".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "native".to_string(),
            wit_source: "registry/example".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: "wit/deps/path-source-0".to_string(),
            wit_digest: digest,
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
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
        let cache_root = dependency_cache::cache_entry_root(&root, "path-source-0");
        copy_tree(
            &root.join("wit/deps/path-source-0"),
            &cache_root.join("wit/deps/path-source-0"),
        );
        copy_tree(
            &root.join("wit/deps/test-dep"),
            &cache_root.join("wit/deps/test-dep"),
        );
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache should be written");

        let err = build_project("default", &root)
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
version = "0.1.0"
kind = "wasm"
path = "registry/example"

[dependencies.component]
path = "registry/example-plugin.wasm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");
        write_file(
            &root.join("registry/example/package.wit"),
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

        let output = build_project("default", &root).expect("build should succeed");
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
version = "0.1.0"
kind = "wasm"
wit = "chikoski:hello"

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
                    wit_source: "chikoski:hello".to_string(),
                    wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    wit_digest: wit_digest.clone(),
                    wit_path: "wit/deps/chikoski-hello".to_string(),
                    component_source: Some("chikoski:hello".to_string()),
                    component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    component_sha256: Some(plugin_sha.clone()),
                }],
                binding_wits: vec![],
                wit_packages: vec![],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "chikoski:hello".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "chikoski:hello".to_string(),
            wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            wit_sha256: None,
            wit_path: "wit/deps/chikoski-hello".to_string(),
            wit_digest,
            wit_source_fingerprint: None,
            component_source: Some("chikoski:hello".to_string()),
            component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            component_sha256: Some(plugin_sha.clone()),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
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

        let output = build_project("default", &root).expect("build should succeed");
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
version = "0.1.0"
kind = "wasm"
wit = "chikoski:hello"

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
                    wit_source: "chikoski:hello".to_string(),
                    wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    wit_digest: wit_digest.clone(),
                    wit_path: "wit/deps/chikoski-hello".to_string(),
                    component_source: Some("chikoski:other".to_string()),
                    component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    component_sha256: Some(plugin_sha.clone()),
                }],
                binding_wits: vec![],
                wit_packages: vec![],
            },
        );
        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "chikoski:hello".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "chikoski:hello".to_string(),
            wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            wit_sha256: None,
            wit_path: "wit/deps/chikoski-hello".to_string(),
            wit_digest,
            wit_source_fingerprint: None,
            component_source: Some("chikoski:hello".to_string()),
            component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
            component_sha256: Some(plugin_sha.clone()),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
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

        let err = build_project("default", &root)
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

        let output = build_project("default", &root).expect("build should succeed");
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

        let output = build_project("default", &root).expect("build should succeed");
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

        let err = build_project("default", &root)
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

        let err = build_project("default", &root).expect_err("backslash path must be rejected");
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

        let err = build_project("default", &root)
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

        let err = build_project("default", &root).expect_err("ca_cert should be rejected");
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

        let err = build_project("default", &root).expect_err("client_cert should be rejected");
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

        let err = build_project("default", &root).expect_err("known_hosts should be rejected");
        assert!(err.to_string().contains("known_hosts"));
        assert!(err.to_string().contains("no longer supported"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_ignores_legacy_secrets_table() {
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
"#,
        );
        write_file(&root.join("build/app.wasm"), b"wasm-a");

        let output = build_project("default", &root).expect("build should succeed");
        let manifest_json = read_manifest_json(&root, &output.manifest_path);
        let object = manifest_json
            .as_object()
            .expect("manifest json root should be object");
        assert!(!object.contains_key("secrets"));

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

        let first = build_project("default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);

        write_file(&root.join("assets/message.txt"), b"hello-updated");

        let second = build_project("default", &root).expect("second build should succeed");
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

        let first = build_project("default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);
        let first_main = assert_hashed_main_path(&first_manifest, "svc");

        let second = build_project("default", &root).expect("second build should succeed");
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

        let first = build_project("default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);
        let first_main = assert_hashed_main_path(&first_manifest, "svc");

        write_file(&root.join("build/app.wasm"), b"wasm-b");

        let second = build_project("default", &root).expect("second build should succeed");
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

        let first = build_project("default", &root).expect("first build should succeed");
        let first_manifest = read_manifest(&root, &first.manifest_path);
        let first_main = assert_hashed_main_path(&first_manifest, "svc");

        write_file(&root.join(&first_main), b"tampered");

        let second = build_project("default", &root).expect("second build should succeed");
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
    fn resolve_manifest_output_path_returns_default_manifest_path() {
        assert_eq!(
            resolve_manifest_output_path(),
            PathBuf::from("build/manifest.json")
        );
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

        let err = build_project("default", &root)
            .expect_err("service name containing path traversal must fail");
        assert!(
            err.to_string()
                .contains("name contains invalid path characters")
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

        let output = build_project("default", &root).expect("build should succeed");
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

            let output = build_project("default", &root).expect("build should succeed");
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

        let err = build_project("default", &root).expect_err("invalid restart policy should fail");
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

        let err =
            build_project("default", &root).expect_err("legacy runtime.restart_policy should fail");
        assert!(err.to_string().contains("runtime.restart_policy"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_service_name_uses_top_level_name() {
        let root = new_temp_dir("load-service-name-top-level");
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

        let default_name = load_service_name(&root).expect("default name should load");

        assert_eq!(default_name, "svc-default");

        let _ = fs::remove_dir_all(root);
    }
}
