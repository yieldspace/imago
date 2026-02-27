use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::{
    cli::{
        ComposeBuildArgs, ComposeCommands, ComposeDeployArgs, ComposeLogsArgs, ComposePsArgs,
        ComposeSubcommandArgs, ComposeUpdateArgs, DeployArgs, LogsArgs, PsArgs, UpdateArgs,
    },
    commands::{
        CommandResult, build, deploy,
        error_diagnostics::{format_command_error, summarize_command_failure},
        logs, ps, ui, update,
    },
};

const COMPOSE_FILE_NAME: &str = "imago-compose.toml";

#[derive(Debug, Deserialize)]
struct ComposeFile {
    #[serde(default)]
    compose: BTreeMap<String, ComposeConfig>,
    #[serde(default)]
    profile: BTreeMap<String, ComposeProfile>,
    #[serde(default)]
    target: BTreeMap<String, ComposeTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposeConfig {
    #[serde(default)]
    services: Vec<ComposeService>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposeProfile {
    config: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposeService {
    imago: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposeTarget {
    remote: String,
    server_name: Option<String>,
    client_key: Option<String>,
}

struct ResolvedComposeConfig<'a> {
    config_name: &'a str,
    config: &'a ComposeConfig,
}

#[derive(Debug)]
struct ComposeCommandError {
    message: String,
}

impl std::fmt::Display for ComposeCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ComposeCommandError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposeSummary {
    subcommand: &'static str,
    profile: String,
    target: String,
    services: usize,
}
fn compose_logs_failure_error(logs_result: &CommandResult) -> ComposeCommandError {
    let detail = logs_result
        .stderr
        .clone()
        .unwrap_or_else(|| format!("exit code {}", logs_result.exit_code));
    ComposeCommandError {
        message: format!("stack logs failed: {detail}"),
    }
}

fn compose_ls_failure_error(ps_result: &CommandResult) -> ComposeCommandError {
    let detail = ps_result
        .stderr
        .clone()
        .unwrap_or_else(|| format!("exit code {}", ps_result.exit_code));
    ComposeCommandError {
        message: format!("stack ls failed: {detail}"),
    }
}

fn should_clear_stack_spinner_before_logs(_follow: bool) -> bool {
    true
}

pub async fn run(args: ComposeSubcommandArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(
    args: ComposeSubcommandArgs,
    project_root: &Path,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("stack", "starting");
    match run_async(args, project_root).await {
        Ok(summary) => {
            ui::command_finish("stack", true, "");
            let mut result = CommandResult::success("stack", started_at);
            result
                .meta
                .insert("subcommand".to_string(), summary.subcommand.to_string());
            result.meta.insert("profile".to_string(), summary.profile);
            result.meta.insert("target".to_string(), summary.target);
            result
                .meta
                .insert("services".to_string(), summary.services.to_string());
            result
        }
        Err(err) => {
            let summary_message = summarize_command_failure("stack", &err);
            let diagnostic_message = format_command_error("stack", &err);
            ui::command_finish("stack", false, &summary_message);
            CommandResult::failure("stack", started_at, diagnostic_message)
        }
    }
}

async fn run_async(
    args: ComposeSubcommandArgs,
    project_root: &Path,
) -> anyhow::Result<ComposeSummary> {
    match args.command {
        ComposeCommands::Build(args) => run_compose_build(args, project_root).await,
        ComposeCommands::Sync(args) => run_compose_update(args, project_root).await,
        ComposeCommands::Deploy(args) => run_compose_deploy(args, project_root).await,
        ComposeCommands::Logs(args) => run_compose_logs(args, project_root).await,
        ComposeCommands::Ls(args) => run_compose_ps(args, project_root).await,
    }
}

async fn run_compose_build(
    args: ComposeBuildArgs,
    project_root: &Path,
) -> anyhow::Result<ComposeSummary> {
    let rich_mode = ui::current_mode() == ui::UiMode::Rich;
    run_compose_build_inner(args, project_root, rich_mode).await
}

async fn run_compose_build_inner(
    args: ComposeBuildArgs,
    project_root: &Path,
    rich_mode: bool,
) -> anyhow::Result<ComposeSummary> {
    ui::command_stage("stack", "load-config", "loading stack build profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let target = resolve_compose_target(&compose_file, &args.target, project_root)?;
    let total = resolved.config.services.len();
    ui::command_info(
        "stack",
        &format!(
            "profile={} target={} services={}",
            args.profile, args.target, total
        ),
    );

    for (index, service) in resolved.config.services.iter().enumerate() {
        let service_name = service.imago.as_str();
        if rich_mode {
            compose_build_service_stage(index + 1, total, service_name);
        } else {
            ui::command_stage(
                "stack",
                "service",
                &format!("build {}/{} {}", index + 1, total, service_name),
            );
        }
        let service_project_root = match resolve_service_project_root(project_root, &service.imago)
            .with_context(|| {
                format!(
                    "failed to resolve compose.{}.services[{index}].imago",
                    resolved.config_name
                )
            }) {
            Ok(path) => path,
            Err(err) => {
                if rich_mode {
                    compose_build_service_finish(service_name, false, &err.to_string());
                }
                return Err(err);
            }
        };

        let build_result = if rich_mode {
            let mut on_build_line = |line: &build::BuildCommandLogLine| {
                compose_build_service_log(service_name, line);
            };
            build::build_project_with_target_override_for_compose(
                &args.target,
                &service_project_root,
                Some(&target),
                Some(&mut on_build_line),
            )
        } else {
            build::build_project_with_target_override_for_compose(
                &args.target,
                &service_project_root,
                Some(&target),
                None,
            )
        };

        match build_result {
            Ok(_) => {
                if rich_mode {
                    compose_build_service_finish(service_name, true, "completed");
                }
            }
            Err(err) => {
                let summary = err.to_string();
                if rich_mode {
                    compose_build_service_finish(service_name, false, &summary);
                }
                let detail = format_command_error("artifact.build", &err);
                return Err(anyhow!(
                    "stack build failed for compose.{}.services[{index}] ({}): {}",
                    resolved.config_name,
                    service_name,
                    detail
                ));
            }
        }
    }

    Ok(ComposeSummary {
        subcommand: "build",
        profile: args.profile,
        target: args.target,
        services: total,
    })
}

fn compose_build_service_stage(index: usize, total: usize, service: &str) {
    #[cfg(test)]
    record_compose_build_ui_event(ComposeBuildUiEvent::Stage {
        service: service.to_string(),
    });
    ui::ensure_compose_service_lines(service);
    ui::compose_build_service_stage(service, "build", &format!("{index}/{total}"));
}

fn compose_build_service_log(service: &str, line: &build::BuildCommandLogLine) {
    #[cfg(test)]
    record_compose_build_ui_event(ComposeBuildUiEvent::Log {
        service: service.to_string(),
        stream: line.stream,
        line: line.line.clone(),
    });
    ui::compose_build_service_log(service, line.stream.as_str(), &line.line);
}

fn compose_build_service_finish(service: &str, succeeded: bool, detail: &str) {
    #[cfg(test)]
    record_compose_build_ui_event(ComposeBuildUiEvent::Finish {
        service: service.to_string(),
        succeeded,
    });
    ui::compose_build_service_finish(service, succeeded, detail);
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ComposeBuildUiEvent {
    Stage {
        service: String,
    },
    Log {
        service: String,
        stream: build::BuildCommandLogStream,
        line: String,
    },
    Finish {
        service: String,
        succeeded: bool,
    },
}

#[cfg(test)]
fn compose_build_ui_events() -> &'static std::sync::Mutex<Vec<ComposeBuildUiEvent>> {
    static EVENTS: std::sync::OnceLock<std::sync::Mutex<Vec<ComposeBuildUiEvent>>> =
        std::sync::OnceLock::new();
    EVENTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[cfg(test)]
fn record_compose_build_ui_event(event: ComposeBuildUiEvent) {
    if let Ok(mut events) = compose_build_ui_events().lock() {
        events.push(event);
    }
}

#[cfg(test)]
fn take_compose_build_ui_events() -> Vec<ComposeBuildUiEvent> {
    if let Ok(mut events) = compose_build_ui_events().lock() {
        return std::mem::take(&mut *events);
    }
    Vec::new()
}

#[cfg(test)]
fn stack_ls_override_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(test)]
fn stack_ls_command_result_override() -> &'static std::sync::Mutex<Option<CommandResult>> {
    static OVERRIDE: std::sync::OnceLock<std::sync::Mutex<Option<CommandResult>>> =
        std::sync::OnceLock::new();
    OVERRIDE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
struct StackLsCommandResultOverrideGuard;

#[cfg(test)]
impl Drop for StackLsCommandResultOverrideGuard {
    fn drop(&mut self) {
        let mut override_result = stack_ls_command_result_override()
            .lock()
            .expect("stack ls command result override lock poisoned");
        *override_result = None;
    }
}

#[cfg(test)]
fn set_stack_ls_command_result_override(
    result: CommandResult,
) -> StackLsCommandResultOverrideGuard {
    let mut override_result = stack_ls_command_result_override()
        .lock()
        .expect("stack ls command result override lock poisoned");
    *override_result = Some(result);
    StackLsCommandResultOverrideGuard
}

fn take_stack_ls_command_result_override() -> Option<CommandResult> {
    #[cfg(test)]
    {
        let mut override_result = stack_ls_command_result_override()
            .lock()
            .expect("stack ls command result override lock poisoned");
        override_result.take()
    }

    #[cfg(not(test))]
    {
        None
    }
}

async fn run_stack_ls_command(
    args: PsArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
    names_filter: Option<Vec<String>>,
) -> CommandResult {
    if let Some(result) = take_stack_ls_command_result_override() {
        return result;
    }
    ps::run_with_project_root_and_target_override(args, project_root, target_override, names_filter)
        .await
}

async fn run_compose_update(
    args: ComposeUpdateArgs,
    project_root: &Path,
) -> anyhow::Result<ComposeSummary> {
    ui::command_stage("stack", "load-config", "loading stack sync profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let total = resolved.config.services.len();

    for (index, service) in resolved.config.services.iter().enumerate() {
        ui::command_stage(
            "stack",
            "service",
            &format!("sync {}/{} {}", index + 1, total, service.imago),
        );
        let service_project_root = resolve_service_project_root(project_root, &service.imago)
            .with_context(|| {
                format!(
                    "failed to resolve compose.{}.services[{index}].imago",
                    resolved.config_name
                )
            })?;

        let update_result =
            update::run_with_project_root(UpdateArgs {}, &service_project_root).await;

        if update_result.exit_code != 0 {
            let detail = update_result
                .stderr
                .unwrap_or_else(|| format!("exit code {}", update_result.exit_code));
            return Err(anyhow!(
                "stack sync failed for compose.{}.services[{index}] ({}): {}",
                resolved.config_name,
                service.imago,
                detail
            ));
        }
    }

    Ok(ComposeSummary {
        subcommand: "sync",
        profile: args.profile,
        target: "-".to_string(),
        services: total,
    })
}

async fn run_compose_deploy(
    args: ComposeDeployArgs,
    project_root: &Path,
) -> anyhow::Result<ComposeSummary> {
    ui::command_stage("stack", "load-config", "loading stack deploy profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let target = resolve_compose_target(&compose_file, &args.target, project_root)?;
    ui::command_info(
        "stack",
        &format!(
            "profile={} target={} services={}",
            args.profile,
            args.target,
            resolved.config.services.len()
        ),
    );
    let total = resolved.config.services.len();

    for (index, service) in resolved.config.services.iter().enumerate() {
        ui::command_stage(
            "stack",
            "service",
            &format!("deploy {}/{} {}", index + 1, total, service.imago),
        );
        let service_project_root = resolve_service_project_root(project_root, &service.imago)
            .with_context(|| {
                format!(
                    "failed to resolve compose.{}.services[{index}].imago",
                    resolved.config_name
                )
            })?;

        let deploy_result = deploy::run_with_project_root_and_target_override(
            DeployArgs {
                target: Some(args.target.clone()),
                detach: true,
            },
            &service_project_root,
            Some(&target),
        )
        .await;

        if deploy_result.exit_code != 0 {
            let detail = deploy_result
                .stderr
                .unwrap_or_else(|| format!("exit code {}", deploy_result.exit_code));
            return Err(anyhow!(
                "stack deploy failed for compose.{}.services[{index}] ({}): {}",
                resolved.config_name,
                service.imago,
                detail
            ));
        }
    }

    Ok(ComposeSummary {
        subcommand: "deploy",
        profile: args.profile,
        target: args.target,
        services: total,
    })
}

async fn run_compose_logs(
    args: ComposeLogsArgs,
    project_root: &Path,
) -> anyhow::Result<ComposeSummary> {
    ui::command_stage("stack", "load-config", "loading stack logs profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let target = resolve_compose_target(&compose_file, &args.target, project_root)?;

    if let Some(name) = &args.name
        && name.trim().is_empty()
    {
        return Err(anyhow!("stack logs --name must not be empty"));
    }

    if should_clear_stack_spinner_before_logs(args.follow) {
        ui::command_clear("stack");
    }

    let logs_result = logs::run_with_project_root_and_target_override(
        LogsArgs {
            name: args.name.clone(),
            follow: args.follow,
            tail: args.tail,
            with_timestamp: args.with_timestamp,
        },
        project_root,
        Some(&target),
    )
    .await;

    if logs_result.exit_code != 0 {
        return Err(compose_logs_failure_error(&logs_result).into());
    }

    Ok(ComposeSummary {
        subcommand: "logs",
        profile: args.profile,
        target: args.target,
        services: resolved.config.services.len(),
    })
}

async fn run_compose_ps(
    args: ComposePsArgs,
    project_root: &Path,
) -> anyhow::Result<ComposeSummary> {
    ui::command_stage("stack", "load-config", "loading stack ls profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let names = resolve_compose_service_names(resolved, project_root)?;
    let services = names.len();
    let target = resolve_compose_target(&compose_file, &args.target, project_root)?;

    let ls_result = run_stack_ls_command(
        PsArgs {
            target: args.target.clone(),
        },
        project_root,
        Some(&target),
        Some(names),
    )
    .await;
    if ls_result.exit_code != 0 {
        return Err(compose_ls_failure_error(&ls_result).into());
    }

    Ok(ComposeSummary {
        subcommand: "ls",
        profile: args.profile,
        target: args.target,
        services,
    })
}

fn resolve_compose_service_names(
    resolved: ResolvedComposeConfig<'_>,
    project_root: &Path,
) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::with_capacity(resolved.config.services.len());
    let mut seen_indices = BTreeMap::<String, usize>::new();

    for (index, service) in resolved.config.services.iter().enumerate() {
        let service_project_root = resolve_service_project_root(project_root, &service.imago)
            .with_context(|| {
                format!(
                    "failed to resolve compose.{}.services[{index}].imago",
                    resolved.config_name
                )
            })?;
        let service_name = build::load_service_name(&service_project_root).with_context(|| {
            format!(
                "failed to resolve compose.{}.services[{index}] service name",
                resolved.config_name
            )
        })?;

        if let Some(previous_index) = seen_indices.insert(service_name.clone(), index) {
            return Err(anyhow!(
                "duplicate service name '{}' in compose.{}.services[{previous_index}] and compose.{}.services[{index}]",
                service_name,
                resolved.config_name,
                resolved.config_name,
            ));
        }

        names.push(service_name);
    }

    Ok(names)
}

fn load_compose_file(project_root: &Path) -> anyhow::Result<ComposeFile> {
    let compose_path = project_root.join(COMPOSE_FILE_NAME);
    let body = fs::read_to_string(&compose_path)
        .with_context(|| format!("failed to read compose file: {}", compose_path.display()))?;
    toml::from_str(&body)
        .with_context(|| format!("failed to parse compose file: {}", compose_path.display()))
}

fn resolve_compose_config<'a>(
    compose_file: &'a ComposeFile,
    profile_name: &str,
) -> anyhow::Result<ResolvedComposeConfig<'a>> {
    let profile = compose_file.profile.get(profile_name).ok_or_else(|| {
        anyhow!(
            "profile '{}' is not defined in {COMPOSE_FILE_NAME}",
            profile_name
        )
    })?;
    let config_name = profile.config.trim();
    if config_name.is_empty() {
        return Err(anyhow!(
            "profile '{}' has empty config in {COMPOSE_FILE_NAME}",
            profile_name
        ));
    }
    let config = compose_file.compose.get(config_name).ok_or_else(|| {
        anyhow!(
            "compose config '{}' is not defined in {COMPOSE_FILE_NAME}",
            config_name
        )
    })?;

    Ok(ResolvedComposeConfig {
        config_name,
        config,
    })
}

fn ensure_compose_services_non_empty(
    config: &ComposeConfig,
    profile_name: &str,
) -> anyhow::Result<()> {
    if config.services.is_empty() {
        return Err(anyhow!(
            "profile '{}' references a compose config with no services in {COMPOSE_FILE_NAME}",
            profile_name
        ));
    }
    Ok(())
}

fn resolve_compose_target(
    compose_file: &ComposeFile,
    target_name: &str,
    project_root: &Path,
) -> anyhow::Result<build::TargetConfig> {
    let target = compose_file.target.get(target_name).ok_or_else(|| {
        anyhow!(
            "target '{}' is not defined in {COMPOSE_FILE_NAME}",
            target_name
        )
    })?;

    let remote = target.remote.trim().to_string();
    if remote.is_empty() {
        return Err(anyhow!(
            "target '{}' is missing required key: remote",
            target_name
        ));
    }

    let server_name = match target.server_name.as_deref() {
        None => None,
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(anyhow!(
                    "target '{}' key 'server_name' must not be empty",
                    target_name
                ));
            }
            Some(trimmed.to_string())
        }
    };

    let client_key = match target.client_key.as_deref() {
        None => None,
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(anyhow!(
                    "target '{}' key 'client_key' must not be empty",
                    target_name
                ));
            }
            Some(build::resolve_target_credential_path(
                trimmed,
                "client_key",
                project_root,
            )?)
        }
    };

    Ok(build::TargetConfig {
        remote,
        server_name,
        client_key,
    })
}

fn resolve_service_project_root(project_root: &Path, imago_path: &str) -> anyhow::Result<PathBuf> {
    let trimmed = imago_path.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("service.imago must not be empty"));
    }

    let relative = Path::new(trimmed);
    let manifest_path = if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        project_root.join(relative)
    };

    if manifest_path.file_name().and_then(|name| name.to_str()) != Some("imago.toml") {
        return Err(anyhow!(
            "service.imago must point to imago.toml (got: {})",
            trimmed
        ));
    }
    if !manifest_path.exists() {
        return Err(anyhow!(
            "service.imago file does not exist: {}",
            manifest_path.display()
        ));
    }
    if !manifest_path.is_file() {
        return Err(anyhow!(
            "service.imago path is not a file: {}",
            manifest_path.display()
        ));
    }

    let parent = manifest_path.parent().ok_or_else(|| {
        anyhow!(
            "service.imago has no parent directory: {}",
            manifest_path.display()
        )
    })?;
    Ok(parent.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("imago-cli-compose-{test_name}-{unique}"));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be created");
        }
        fs::write(path, bytes).expect("file should be written");
    }

    #[tokio::test]
    async fn returns_non_zero_when_imago_compose_toml_is_missing() {
        let root = new_temp_dir("missing-compose");
        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Deploy(ComposeDeployArgs {
                    profile: "mini".to_string(),
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be present")
                .contains("failed to read compose file")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn returns_non_zero_when_profile_is_missing() {
        let root = new_temp_dir("missing-profile");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[profile.dev]
config = "missing"
"#,
        );

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Sync(ComposeUpdateArgs {
                    profile: "prod".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be present")
                .contains("profile 'prod' is not defined")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn compose_build_succeeds_with_compose_target_when_service_has_no_target() {
        let root = new_temp_dir("build-with-compose-target");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[profile.dev]
config = "stack"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"
"#,
        );
        write_file(&root.join("services/svc-a/build/app.wasm"), b"\0asm");

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Build(ComposeBuildArgs {
                    profile: "dev".to_string(),
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 0);
        assert!(root.join("services/svc-a/build/manifest.json").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn compose_deploy_uses_compose_target_override_and_requires_client_key() {
        let root = new_temp_dir("deploy-compose-target-override");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[profile.dev]
config = "stack"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"
"#,
        );
        write_file(&root.join("services/svc-a/build/app.wasm"), b"\0asm");

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Deploy(ComposeDeployArgs {
                    profile: "dev".to_string(),
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.expect("stderr should be present");
        assert!(stderr.contains("stack deploy failed"));
        assert!(stderr.contains("target settings are invalid for service deploy"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn compose_logs_requires_defined_compose_target() {
        let root = new_temp_dir("logs-missing-target");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[profile.dev]
config = "stack"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"
"#,
        );

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Logs(ComposeLogsArgs {
                    profile: "dev".to_string(),
                    target: "default".to_string(),
                    name: Some("svc-a".to_string()),
                    follow: false,
                    tail: 10,
                    with_timestamp: false,
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be present")
                .contains("target 'default' is not defined")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clear_stack_spinner_before_logs_when_follow_enabled() {
        assert!(should_clear_stack_spinner_before_logs(true));
    }

    #[test]
    fn clear_stack_spinner_before_logs_when_follow_disabled() {
        assert!(should_clear_stack_spinner_before_logs(false));
    }

    #[tokio::test]
    async fn compose_ps_rejects_duplicate_service_names_in_profile() {
        let root = new_temp_dir("ps-duplicate-service-names");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[[compose.stack.services]]
imago = "services/svc-b/imago.toml"

[profile.dev]
config = "stack"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-dup"
main = "build/app.wasm"
type = "cli"
"#,
        );
        write_file(
            &root.join("services/svc-b/imago.toml"),
            br#"
name = "svc-dup"
main = "build/app.wasm"
type = "cli"
"#,
        );

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Ls(ComposePsArgs {
                    profile: "dev".to_string(),
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be present")
                .contains("duplicate service name")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn compose_build_rejects_target_client_key_path_traversal() {
        let root = new_temp_dir("target-client-key-path-traversal");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[profile.dev]
config = "stack"

[target.default]
remote = "127.0.0.1:4443"
client_key = "../certs/client.key"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"
"#,
        );
        write_file(&root.join("services/svc-a/build/app.wasm"), b"\0asm");

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Build(ComposeBuildArgs {
                    profile: "dev".to_string(),
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be present")
                .contains("must not contain path traversal")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn compose_update_runs_for_profile_services() {
        let root = new_temp_dir("update-services");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[profile.dev]
config = "stack"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"
"#,
        );

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Sync(ComposeUpdateArgs {
                    profile: "dev".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.command, "stack");
        assert_eq!(
            result.meta.get("subcommand").map(String::as_str),
            Some("sync")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn compose_ls_reports_stack_command_metadata() {
        let _lock = stack_ls_override_test_lock().lock().await;
        let root = new_temp_dir("ls-stack-command-metadata");
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[profile.dev]
config = "stack"

[target.default]
remote = "127.0.0.1:4443"
client_key = "certs/client.key"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"
"#,
        );
        write_file(&root.join("certs/client.key"), b"dummy-client-key");

        let _guard = set_stack_ls_command_result_override(CommandResult {
            command: "service.ls".to_string(),
            exit_code: 0,
            stderr: None,
            duration_ms: 0,
            meta: BTreeMap::new(),
        });

        let result = run_with_project_root(
            ComposeSubcommandArgs {
                command: ComposeCommands::Ls(ComposePsArgs {
                    profile: "dev".to_string(),
                    target: "default".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.command, "stack");
        assert_eq!(
            result.meta.get("subcommand").map(String::as_str),
            Some("ls")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn compose_ls_override_guard_clears_override_on_panic() {
        let _lock = stack_ls_override_test_lock().lock().await;
        let _ = std::panic::catch_unwind(|| {
            let _guard = set_stack_ls_command_result_override(CommandResult {
                command: "service.ls".to_string(),
                exit_code: 1,
                stderr: Some("injected failure".to_string()),
                duration_ms: 0,
                meta: BTreeMap::new(),
            });
            panic!("intentional panic");
        });

        let result = take_stack_ls_command_result_override();
        assert!(result.is_none(), "override must be cleared by guard drop");
    }

    #[tokio::test]
    async fn compose_build_uses_build_line_capture_only_in_rich_mode() {
        let root = new_temp_dir("build-callback-rich-only");
        let _ = take_compose_build_ui_events();
        write_file(
            &root.join(COMPOSE_FILE_NAME),
            br#"
[[compose.stack.services]]
imago = "services/svc-a/imago.toml"

[profile.dev]
config = "stack"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write_file(
            &root.join("services/svc-a/imago.toml"),
            br#"
name = "svc-a"
main = "build/app.wasm"
type = "cli"

[build]
command = ["sh", "-c", "printf 'before-fail\n'; printf 'err-before-fail\n' >&2; exit 7"]
"#,
        );

        run_compose_build_inner(
            ComposeBuildArgs {
                profile: "dev".to_string(),
                target: "default".to_string(),
            },
            &root,
            true,
        )
        .await
        .expect_err("rich mode compose build should fail");
        let rich_events = take_compose_build_ui_events();
        assert!(
            rich_events
                .iter()
                .any(|event| matches!(event, ComposeBuildUiEvent::Stage { .. }))
        );
        assert!(rich_events.iter().any(|event| {
            matches!(
                event,
                ComposeBuildUiEvent::Log {
                    stream: build::BuildCommandLogStream::Stdout,
                    line,
                    ..
                } if line == "before-fail"
            )
        }));
        assert!(rich_events.iter().any(|event| {
            matches!(
                event,
                ComposeBuildUiEvent::Log {
                    stream: build::BuildCommandLogStream::Stderr,
                    line,
                    ..
                } if line == "err-before-fail"
            )
        }));
        assert!(rich_events.iter().any(|event| {
            matches!(
                event,
                ComposeBuildUiEvent::Finish {
                    succeeded: false,
                    ..
                }
            )
        }));

        run_compose_build_inner(
            ComposeBuildArgs {
                profile: "dev".to_string(),
                target: "default".to_string(),
            },
            &root,
            false,
        )
        .await
        .expect_err("plain mode compose build should fail");
        assert!(take_compose_build_ui_events().is_empty());

        let _ = fs::remove_dir_all(root);
    }
}
