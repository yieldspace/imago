use std::{path::Path, time::Instant};

use anyhow::Context;
use imago_protocol::{CommandPayload, CommandType, RunCommandPayload};
use uuid::Uuid;

use crate::{
    cli::{LogsArgs, RunArgs},
    commands::{
        CommandResult, build,
        command_common::{
            format_local_context_line, format_peer_context_line, handle_terminal_event,
            negotiate_hello, resolve_service_name,
        },
        deploy,
        error_diagnostics::{self, summarize_command_failure},
        logs, ui,
    },
};

const AUTO_FOLLOW_TAIL_LINES: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunSummary {
    service_name: String,
    target_name: String,
    detach: bool,
}

pub async fn run(args: RunArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(args: RunArgs, project_root: &Path) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("service.start", "starting");
    match run_async(args, project_root).await {
        Ok(summary) => {
            ui::command_finish("service.start", true, "");
            let mut result = CommandResult::success("service.start", started_at);
            result
                .meta
                .insert("service".to_string(), summary.service_name);
            result
                .meta
                .insert("target".to_string(), summary.target_name);
            result
                .meta
                .insert("detach".to_string(), summary.detach.to_string());
            result
        }
        Err(err) => {
            let summary = summarize_command_failure("service.start", &err);
            ui::command_finish("service.start", false, &summary);
            let message = error_diagnostics::format_command_error("service.start", &err);
            CommandResult::failure("service.start", started_at, message)
        }
    }
}

async fn run_async(args: RunArgs, project_root: &Path) -> anyhow::Result<RunSummary> {
    let RunArgs {
        name,
        target,
        detach,
    } = args;
    ui::command_stage(
        "service.start",
        "load-config",
        "loading target configuration",
    );
    let target_name = target.unwrap_or_else(|| build::default_target_name().to_string());
    let target_config = build::load_target_config(&target_name, project_root)
        .context("failed to load target configuration")?;
    let target = target_config
        .require_deploy_credentials()
        .context("target settings are invalid for service start")?;
    let service_name = resolve_service_name(name.as_deref(), project_root)
        .context("failed to resolve service name for service start")?;
    ui::command_info(
        "service.start",
        &format_local_context_line(
            project_root,
            &service_name,
            &target_name,
            &target.remote,
            target.server_name.as_deref(),
        ),
    );

    ui::command_stage("service.start", "connect", "connecting target");
    let connected = deploy::connect_target(&target).await?;
    let correlation_id = Uuid::new_v4();
    ui::command_stage("service.start", "hello", "negotiating hello");
    let hello = negotiate_hello(&connected.session, correlation_id).await?;
    ui::command_info(
        "service.start",
        &format_peer_context_line(
            &connected.authority,
            &connected.resolved_addr.to_string(),
            &hello,
        ),
    );
    let command_stream_timeout =
        deploy::resolve_command_stream_timeout_from_hello_limits(&hello.limits);

    ui::command_stage("service.start", "command.start", "sending run request");
    let command = deploy::build_command_start_envelope(
        correlation_id,
        Uuid::new_v4(),
        CommandType::Run,
        CommandPayload::Run(RunCommandPayload {
            name: service_name.clone(),
        }),
    )?;
    let responses = deploy::request_command_start_events_with_timeout(
        &connected.session,
        &command,
        command_stream_timeout,
    )
    .await?;
    handle_terminal_event("service.start", responses)?;
    if !detach {
        follow_logs_after_run(project_root, &target_config, &service_name).await;
    }
    Ok(RunSummary {
        service_name,
        target_name,
        detach,
    })
}

async fn follow_logs_after_run(
    project_root: &Path,
    target_config: &build::TargetConfig,
    service_name: &str,
) {
    let logs_result = logs::run_with_project_root_and_target_override(
        LogsArgs {
            name: Some(service_name.to_string()),
            follow: true,
            tail: AUTO_FOLLOW_TAIL_LINES,
            with_timestamp: false,
        },
        project_root,
        Some(target_config),
    )
    .await;
    if logs_result.exit_code != 0 {
        let detail = logs_result
            .stderr
            .unwrap_or_else(|| format!("exit code {}", logs_result.exit_code));
        ui::command_warn(
            "service.start",
            &format!("service logs --follow failed after service start succeeded: {detail}"),
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::commands::command_common::resolve_service_name;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("imago-cli-run-{test_name}-{unique}"));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }

    fn write_imago_toml(project_root: &Path, content: &str) {
        fs::write(project_root.join("imago.toml"), content).expect("imago.toml should be written");
    }

    #[test]
    fn resolve_service_name_accepts_explicit_valid_name() {
        let root = new_temp_dir("resolve-explicit-valid");
        let name =
            resolve_service_name(Some("svc-a"), &root).expect("explicit name should resolve");
        assert_eq!(name, "svc-a");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_service_name_rejects_explicit_path_traversal() {
        let root = new_temp_dir("resolve-explicit-dotdot");
        let err = resolve_service_name(Some("../foo"), &root)
            .expect_err("path traversal name should be rejected");
        assert!(
            err.to_string()
                .contains("name contains invalid path characters")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_service_name_rejects_explicit_forward_slash() {
        let root = new_temp_dir("resolve-explicit-slash");
        let err = resolve_service_name(Some("svc/foo"), &root)
            .expect_err("slash name should be rejected");
        assert!(
            err.to_string()
                .contains("name contains invalid path characters")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_service_name_rejects_explicit_backslash() {
        let root = new_temp_dir("resolve-explicit-backslash");
        let err = resolve_service_name(Some("svc\\foo"), &root)
            .expect_err("backslash name should be rejected");
        assert!(
            err.to_string()
                .contains("name contains invalid path characters")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_service_name_uses_default_name_when_explicit_is_none() {
        let root = new_temp_dir("resolve-from-default");
        write_imago_toml(
            &root,
            r#"
name = "svc-default"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let name = resolve_service_name(None, &root).expect("default service name should resolve");
        assert_eq!(name, "svc-default");

        let _ = fs::remove_dir_all(root);
    }
}
