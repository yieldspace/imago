use std::{path::Path, time::Instant};

use anyhow::Context;
use imago_protocol::{CommandPayload, CommandType, StopCommandPayload};
use uuid::Uuid;

use crate::{
    cli::StopArgs,
    commands::{
        CommandResult, build,
        command_common::{
            format_local_context_line, format_peer_context_line, handle_terminal_event,
            negotiate_hello, resolve_service_name,
        },
        deploy, ui,
    },
};

pub async fn run(args: StopArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(args: StopArgs, project_root: &Path) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("stop", "starting");
    match run_async(args, project_root).await {
        Ok(()) => {
            ui::command_finish("stop", true, "completed");
            CommandResult::success("stop", started_at)
        }
        Err(err) => {
            let message = err.to_string();
            ui::command_finish("stop", false, &message);
            CommandResult::failure("stop", started_at, message)
        }
    }
}

async fn run_async(args: StopArgs, project_root: &Path) -> anyhow::Result<()> {
    ui::command_stage("stop", "load-config", "loading target configuration");
    let target_name = args
        .target
        .clone()
        .unwrap_or_else(|| build::default_target_name().to_string());
    let target = build::load_target_config(&target_name, project_root)
        .context("failed to load target configuration")?
        .require_deploy_credentials()
        .context("target settings are invalid for stop")?;
    let service_name = resolve_service_name(args.name.as_deref(), project_root)
        .context("failed to resolve service name for stop")?;
    ui::command_info(
        "stop",
        &format_local_context_line(
            project_root,
            &service_name,
            &target_name,
            &target.remote,
            target.server_name.as_deref(),
        ),
    );

    ui::command_stage("stop", "connect", "connecting target");
    let connected = deploy::connect_target(&target).await?;
    let correlation_id = Uuid::new_v4();
    ui::command_stage("stop", "hello", "negotiating hello");
    let hello = negotiate_hello(&connected.session, correlation_id).await?;
    ui::command_info(
        "stop",
        &format_peer_context_line(
            &connected.authority,
            &connected.resolved_addr.to_string(),
            &hello,
        ),
    );

    ui::command_stage("stop", "command.start", "sending stop request");
    let command = deploy::build_command_start_envelope(
        correlation_id,
        Uuid::new_v4(),
        CommandType::Stop,
        CommandPayload::Stop(StopCommandPayload {
            name: service_name,
            force: args.force,
        }),
    )?;
    let responses = deploy::request_events(&connected.session, &command).await?;
    handle_terminal_event("stop", responses)
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
        let dir = std::env::temp_dir().join(format!("imago-cli-stop-{test_name}-{unique}"));
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
