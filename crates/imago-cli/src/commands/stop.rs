use std::path::Path;

use anyhow::Context;
use imago_protocol::{CommandPayload, CommandType, StopCommandPayload};
use uuid::Uuid;

use crate::{
    cli::StopArgs,
    commands::{
        CommandResult, build,
        command_common::{handle_terminal_event, negotiate_hello, resolve_service_name},
        deploy,
    },
};

pub fn run(args: StopArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(args: StopArgs, project_root: &Path) -> CommandResult {
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

fn run_inner(args: StopArgs, project_root: &Path) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;
    runtime.block_on(run_async(args, project_root))
}

async fn run_async(args: StopArgs, project_root: &Path) -> anyhow::Result<()> {
    let target_name = args
        .target
        .clone()
        .unwrap_or_else(|| build::default_target_name().to_string());
    let target = build::load_target_config(args.env.as_deref(), &target_name, project_root)
        .context("failed to load target configuration")?
        .require_deploy_credentials()
        .context("target settings are invalid for stop")?;
    let service_name =
        resolve_service_name(args.name.as_deref(), args.env.as_deref(), project_root)
            .context("failed to resolve service name for stop")?;

    let session = deploy::connect_target(&target).await?;
    let correlation_id = Uuid::new_v4();
    negotiate_hello(&session, correlation_id).await?;

    let command = deploy::build_command_start_envelope(
        correlation_id,
        Uuid::new_v4(),
        CommandType::Stop,
        CommandPayload::Stop(StopCommandPayload {
            name: service_name,
            force: args.force,
        }),
    )?;
    let responses = deploy::request_events(&session, &command).await?;
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
            resolve_service_name(Some("svc-a"), None, &root).expect("explicit name should resolve");
        assert_eq!(name, "svc-a");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_service_name_rejects_explicit_path_traversal() {
        let root = new_temp_dir("resolve-explicit-dotdot");
        let err = resolve_service_name(Some("../foo"), None, &root)
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
        let err = resolve_service_name(Some("svc/foo"), None, &root)
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
        let err = resolve_service_name(Some("svc\\foo"), None, &root)
            .expect_err("backslash name should be rejected");
        assert!(
            err.to_string()
                .contains("name contains invalid path characters")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_service_name_uses_env_name_when_explicit_is_none() {
        let root = new_temp_dir("resolve-from-env");
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

        let name = resolve_service_name(None, Some("prod"), &root)
            .expect("env override service name should resolve");
        assert_eq!(name, "svc-prod");

        let _ = fs::remove_dir_all(root);
    }
}
