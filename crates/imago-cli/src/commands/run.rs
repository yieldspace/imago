use std::path::Path;

use anyhow::{Context, anyhow};
use imago_protocol::{
    CommandEvent, CommandEventType, CommandPayload, CommandStartResponse, CommandType,
    HelloNegotiateRequest, HelloNegotiateResponse, MessageType, RunCommandPayload,
};
use uuid::Uuid;

use crate::{
    cli::RunArgs,
    commands::{CommandResult, build, deploy},
};

const HELLO_REQUIRED_FEATURES: [&str; 2] = ["command.start", "command.event"];

pub fn run(args: RunArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(args: RunArgs, project_root: &Path) -> CommandResult {
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

fn run_inner(args: RunArgs, project_root: &Path) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;
    runtime.block_on(run_async(args, project_root))
}

async fn run_async(args: RunArgs, project_root: &Path) -> anyhow::Result<()> {
    let target_name = args
        .target
        .clone()
        .unwrap_or_else(|| build::default_target_name().to_string());
    let target = build::load_target_config(args.env.as_deref(), &target_name, project_root)
        .context("failed to load target configuration")?
        .require_deploy_credentials()
        .context("target settings are invalid for run")?;
    let service_name =
        resolve_service_name(args.name.as_deref(), args.env.as_deref(), project_root)
            .context("failed to resolve service name for run")?;

    let session = deploy::connect_target(&target).await?;
    let correlation_id = Uuid::new_v4();
    negotiate_hello(&session, correlation_id).await?;

    let command = deploy::build_command_start_envelope(
        correlation_id,
        Uuid::new_v4(),
        CommandType::Run,
        CommandPayload::Run(RunCommandPayload { name: service_name }),
    )?;
    let responses = deploy::request_events(&session, &command).await?;
    handle_terminal_event("run", responses)
}

fn resolve_service_name(
    explicit_name: Option<&str>,
    env: Option<&str>,
    project_root: &Path,
) -> anyhow::Result<String> {
    if let Some(name) = explicit_name {
        let trimmed = name.trim();
        build::validate_service_name(trimmed)?;
        return Ok(trimmed.to_string());
    }
    build::load_service_name(env, project_root)
}

async fn negotiate_hello(
    session: &web_transport_quinn::Session,
    correlation_id: Uuid,
) -> anyhow::Result<()> {
    let hello_request = deploy::request_envelope(
        MessageType::HelloNegotiate,
        Uuid::new_v4(),
        correlation_id,
        &HelloNegotiateRequest {
            compatibility_date: deploy::COMPATIBILITY_DATE.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            required_features: HELLO_REQUIRED_FEATURES
                .iter()
                .map(|feature| feature.to_string())
                .collect(),
        },
    )?;
    let hello_response: HelloNegotiateResponse =
        deploy::response_payload(deploy::request_response(session, &hello_request).await?)?;
    if hello_response.accepted {
        return Ok(());
    }
    Err(anyhow!("hello.negotiate was rejected by server"))
}

fn handle_terminal_event(
    command_name: &str,
    responses: Vec<deploy::Envelope>,
) -> anyhow::Result<()> {
    if responses.is_empty() {
        return Err(anyhow!("command.start returned empty response stream"));
    }

    let start_response: CommandStartResponse = deploy::response_payload(responses[0].clone())?;
    if !start_response.accepted {
        return Err(anyhow!("command.start was not accepted"));
    }

    let mut terminal: Option<CommandEvent> = None;
    for envelope in responses.iter().skip(1) {
        if envelope.message_type != MessageType::CommandEvent {
            continue;
        }
        let event: CommandEvent = deploy::response_payload(envelope.clone())?;
        if matches!(
            event.event_type,
            CommandEventType::Succeeded | CommandEventType::Failed | CommandEventType::Canceled
        ) {
            terminal = Some(event);
            break;
        }
    }

    let terminal =
        terminal.ok_or_else(|| anyhow!("command.event terminal event was not received"))?;

    match terminal.event_type {
        CommandEventType::Succeeded => Ok(()),
        CommandEventType::Failed => {
            if let Some(err) = terminal.error {
                Err(anyhow!(
                    "{} failed: {} ({:?}) at {}",
                    command_name,
                    err.message,
                    err.code,
                    err.stage
                ))
            } else {
                Err(anyhow!("{command_name} failed without structured error"))
            }
        }
        CommandEventType::Canceled => Err(anyhow!("{command_name} was canceled")),
        _ => Err(anyhow!("unexpected terminal event")),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_service_name;
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
