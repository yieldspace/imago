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
        BuildArgs, ComposeBuildArgs, ComposeCommands, ComposeDeployArgs, ComposeLogsArgs,
        ComposeSubcommandArgs, ComposeUpdateArgs, DeployArgs, LogsArgs, UpdateArgs,
    },
    commands::{CommandResult, build, deploy, logs, ui, update},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposeRunKind {
    Build,
    Update,
    Deploy,
    Logs,
}

impl ComposeRunKind {
    fn from_command(command: &ComposeCommands) -> Self {
        match command {
            ComposeCommands::Build(_) => Self::Build,
            ComposeCommands::Update(_) => Self::Update,
            ComposeCommands::Deploy(_) => Self::Deploy,
            ComposeCommands::Logs(_) => Self::Logs,
        }
    }
}

#[derive(Debug)]
struct ComposeCommandError {
    message: String,
    suppress_json_summary: bool,
}

impl ComposeCommandError {
    fn suppresses_json_summary(&self) -> bool {
        self.suppress_json_summary
    }
}

impl std::fmt::Display for ComposeCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ComposeCommandError {}

fn should_suppress_json_summary_for_compose_success(
    run_kind: ComposeRunKind,
    ui_mode: ui::UiMode,
) -> bool {
    run_kind == ComposeRunKind::Logs && ui_mode == ui::UiMode::Json
}

fn delegated_logs_failure_already_emitted_json_error(
    logs_result: &CommandResult,
    ui_mode: ui::UiMode,
) -> bool {
    ui_mode == ui::UiMode::Json
        && logs_result.exit_code != 0
        && logs_result.skip_json_summary
        && logs_result.stderr.is_none()
}

fn compose_logs_failure_error(
    logs_result: &CommandResult,
    ui_mode: ui::UiMode,
) -> ComposeCommandError {
    let detail = logs_result
        .stderr
        .clone()
        .unwrap_or_else(|| format!("exit code {}", logs_result.exit_code));
    ComposeCommandError {
        message: format!("compose logs failed: {detail}"),
        suppress_json_summary: delegated_logs_failure_already_emitted_json_error(
            logs_result,
            ui_mode,
        ),
    }
}

fn should_suppress_json_summary_for_compose_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<ComposeCommandError>()
            .map(ComposeCommandError::suppresses_json_summary)
            .unwrap_or(false)
    })
}

pub async fn run(args: ComposeSubcommandArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(
    args: ComposeSubcommandArgs,
    project_root: &Path,
) -> CommandResult {
    let started_at = Instant::now();
    let run_kind = ComposeRunKind::from_command(&args.command);
    let ui_mode = ui::current_mode();
    ui::command_start("compose", "starting");
    match run_async(args, project_root).await {
        Ok(()) => {
            ui::command_finish("compose", true, "completed");
            let mut result = CommandResult::success("compose", started_at);
            if should_suppress_json_summary_for_compose_success(run_kind, ui_mode) {
                result = result.without_json_summary();
            }
            result
        }
        Err(err) => {
            let message = err.to_string();
            ui::command_finish("compose", false, &message);
            let mut result = CommandResult::failure("compose", started_at, message);
            if should_suppress_json_summary_for_compose_error(&err) {
                result = result.without_json_summary();
            }
            result
        }
    }
}

async fn run_async(args: ComposeSubcommandArgs, project_root: &Path) -> anyhow::Result<()> {
    match args.command {
        ComposeCommands::Build(args) => run_compose_build(args, project_root).await,
        ComposeCommands::Update(args) => run_compose_update(args, project_root).await,
        ComposeCommands::Deploy(args) => run_compose_deploy(args, project_root).await,
        ComposeCommands::Logs(args) => run_compose_logs(args, project_root).await,
    }
}

async fn run_compose_build(args: ComposeBuildArgs, project_root: &Path) -> anyhow::Result<()> {
    ui::command_stage("compose", "load-config", "loading compose build profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let target = resolve_compose_target(&compose_file, &args.target, project_root)?;
    let total = resolved.config.services.len();
    ui::command_info(
        "compose",
        &format!(
            "profile={} target={} services={}",
            args.profile, args.target, total
        ),
    );

    for (index, service) in resolved.config.services.iter().enumerate() {
        ui::command_stage(
            "compose",
            "service",
            &format!("build {}/{} {}", index + 1, total, service.imago),
        );
        let service_project_root = resolve_service_project_root(project_root, &service.imago)
            .with_context(|| {
                format!(
                    "failed to resolve compose.{}.services[{index}].imago",
                    resolved.config_name
                )
            })?;

        let build_result = build::run_with_project_root_and_target_override(
            BuildArgs {
                target: args.target.clone(),
            },
            &service_project_root,
            Some(&target),
        );

        if build_result.exit_code != 0 {
            let detail = build_result
                .stderr
                .unwrap_or_else(|| format!("exit code {}", build_result.exit_code));
            return Err(anyhow!(
                "compose build failed for compose.{}.services[{index}] ({}): {}",
                resolved.config_name,
                service.imago,
                detail
            ));
        }
    }

    Ok(())
}

async fn run_compose_update(args: ComposeUpdateArgs, project_root: &Path) -> anyhow::Result<()> {
    ui::command_stage("compose", "load-config", "loading compose update profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let total = resolved.config.services.len();

    for (index, service) in resolved.config.services.iter().enumerate() {
        ui::command_stage(
            "compose",
            "service",
            &format!("update {}/{} {}", index + 1, total, service.imago),
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
                "compose update failed for compose.{}.services[{index}] ({}): {}",
                resolved.config_name,
                service.imago,
                detail
            ));
        }
    }

    Ok(())
}

async fn run_compose_deploy(args: ComposeDeployArgs, project_root: &Path) -> anyhow::Result<()> {
    ui::command_stage("compose", "load-config", "loading compose deploy profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let target = resolve_compose_target(&compose_file, &args.target, project_root)?;
    ui::command_info(
        "compose",
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
            "compose",
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
                "compose deploy failed for compose.{}.services[{index}] ({}): {}",
                resolved.config_name,
                service.imago,
                detail
            ));
        }
    }

    Ok(())
}

async fn run_compose_logs(args: ComposeLogsArgs, project_root: &Path) -> anyhow::Result<()> {
    ui::command_stage("compose", "load-config", "loading compose logs profile");
    let compose_file = load_compose_file(project_root)?;
    let resolved = resolve_compose_config(&compose_file, &args.profile)?;
    ensure_compose_services_non_empty(resolved.config, &args.profile)?;
    let target = resolve_compose_target(&compose_file, &args.target, project_root)?;

    if let Some(name) = &args.name
        && name.trim().is_empty()
    {
        return Err(anyhow!("compose logs --name must not be empty"));
    }

    let logs_result = logs::run_with_project_root_and_target_override(
        LogsArgs {
            name: args.name.clone(),
            follow: args.follow,
            tail: args.tail,
        },
        project_root,
        Some(&target),
    )
    .await;

    if logs_result.exit_code != 0 {
        return Err(compose_logs_failure_error(&logs_result, ui::current_mode()).into());
    }

    Ok(())
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

    #[test]
    fn compose_logs_success_suppresses_json_summary_only_for_json_mode() {
        assert!(should_suppress_json_summary_for_compose_success(
            ComposeRunKind::Logs,
            ui::UiMode::Json
        ));
        assert!(!should_suppress_json_summary_for_compose_success(
            ComposeRunKind::Logs,
            ui::UiMode::Plain
        ));
        assert!(!should_suppress_json_summary_for_compose_success(
            ComposeRunKind::Build,
            ui::UiMode::Json
        ));
    }

    #[test]
    fn compose_logs_json_delegate_failure_marks_outer_summary_suppressed() {
        let logs_result = CommandResult {
            command: "logs".to_string(),
            exit_code: 2,
            stderr: None,
            duration_ms: 0,
            meta: BTreeMap::new(),
            skip_json_summary: true,
        };

        let err: anyhow::Error = compose_logs_failure_error(&logs_result, ui::UiMode::Json).into();
        assert!(should_suppress_json_summary_for_compose_error(&err));
    }

    #[test]
    fn compose_logs_non_json_delegate_failure_keeps_outer_summary() {
        let logs_result = CommandResult {
            command: "logs".to_string(),
            exit_code: 2,
            stderr: Some("network failed".to_string()),
            duration_ms: 0,
            meta: BTreeMap::new(),
            skip_json_summary: true,
        };

        let err: anyhow::Error = compose_logs_failure_error(&logs_result, ui::UiMode::Plain).into();
        assert!(!should_suppress_json_summary_for_compose_error(&err));
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
                command: ComposeCommands::Update(ComposeUpdateArgs {
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
        assert!(stderr.contains("compose deploy failed"));
        assert!(stderr.contains("target settings are invalid for deploy"));

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
                command: ComposeCommands::Update(ComposeUpdateArgs {
                    profile: "dev".to_string(),
                }),
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 0);

        let _ = fs::remove_dir_all(root);
    }
}
