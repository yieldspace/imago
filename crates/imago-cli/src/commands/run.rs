use std::{path::Path, time::Instant};

use anyhow::Context;
use imago_protocol::{CommandPayload, CommandType, RunCommandPayload};
use uuid::Uuid;

use crate::{
    cli::RunArgs,
    commands::{
        CommandResult, build,
        command_common::{
            format_local_context_line, format_peer_context_line, handle_terminal_event,
            negotiate_hello, resolve_service_name,
        },
        deploy, error_diagnostics, ui,
    },
};

pub async fn run(args: RunArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(args: RunArgs, project_root: &Path) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("run", "starting");
    match run_async(args, project_root).await {
        Ok(()) => {
            ui::command_finish("run", true, "completed");
            CommandResult::success("run", started_at)
        }
        Err(err) => {
            let summary = err.to_string();
            ui::command_finish("run", false, &summary);
            let message = error_diagnostics::format_command_error("run", &err);
            CommandResult::failure("run", started_at, message)
        }
    }
}

async fn run_async(args: RunArgs, project_root: &Path) -> anyhow::Result<()> {
    ui::command_stage("run", "load-config", "loading target configuration");
    let target_name = args
        .target
        .clone()
        .unwrap_or_else(|| build::default_target_name().to_string());
    let target = build::load_target_config(&target_name, project_root)
        .context("failed to load target configuration")?
        .require_deploy_credentials()
        .context("target settings are invalid for run")?;
    let service_name = resolve_service_name(args.name.as_deref(), project_root)
        .context("failed to resolve service name for run")?;
    ui::command_info(
        "run",
        &format_local_context_line(
            project_root,
            &service_name,
            &target_name,
            &target.remote,
            target.server_name.as_deref(),
        ),
    );

    ui::command_stage("run", "connect", "connecting target");
    let connected = deploy::connect_target(&target).await?;
    let correlation_id = Uuid::new_v4();
    ui::command_stage("run", "hello", "negotiating hello");
    let hello = negotiate_hello(&connected.session, correlation_id).await?;
    ui::command_info(
        "run",
        &format_peer_context_line(
            &connected.authority,
            &connected.resolved_addr.to_string(),
            &hello,
        ),
    );
    let command_stream_timeout =
        deploy::resolve_command_stream_timeout_from_hello_limits(&hello.limits);

    ui::command_stage("run", "command.start", "sending run request");
    let command = deploy::build_command_start_envelope(
        correlation_id,
        Uuid::new_v4(),
        CommandType::Run,
        CommandPayload::Run(RunCommandPayload { name: service_name }),
    )?;
    let responses = deploy::request_command_start_events_with_timeout(
        &connected.session,
        &command,
        command_stream_timeout,
    )
    .await?;
    handle_terminal_event("run", responses)
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
