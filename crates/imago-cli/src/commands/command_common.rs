use std::{collections::BTreeMap, path::Path};

use anyhow::anyhow;
use imago_protocol::{
    CommandEvent, CommandEventType, CommandStartResponse, HelloNegotiateRequest,
    HelloNegotiateResponse, MessageType, PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSION_RANGE,
};
use semver::{Version, VersionReq};
use uuid::Uuid;

use crate::commands::ui;
use crate::commands::{build, deploy};

const HELLO_REQUIRED_FEATURES: [&str; 2] = ["command.start", "command.event"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelloSummary {
    pub server_version: String,
    pub features: Vec<String>,
    pub limits: BTreeMap<String, String>,
}

pub(crate) fn resolve_service_name(
    explicit_name: Option<&str>,
    project_root: &Path,
) -> anyhow::Result<String> {
    if let Some(name) = explicit_name {
        let trimmed = name.trim();
        build::validate_service_name(trimmed)?;
        return Ok(trimmed.to_string());
    }
    build::load_service_name(project_root)
}

pub(crate) async fn negotiate_hello(
    session: &web_transport_quinn::Session,
    correlation_id: Uuid,
) -> anyhow::Result<HelloSummary> {
    negotiate_hello_with_features(session, correlation_id, &HELLO_REQUIRED_FEATURES).await
}

pub(crate) async fn negotiate_hello_with_features(
    session: &web_transport_quinn::Session,
    correlation_id: Uuid,
    required_features: &[&str],
) -> anyhow::Result<HelloSummary> {
    let hello_request = deploy::request_envelope(
        MessageType::HelloNegotiate,
        Uuid::new_v4(),
        correlation_id,
        &HelloNegotiateRequest {
            client_version: PROTOCOL_VERSION.to_string(),
            required_features: required_features
                .iter()
                .map(|feature| feature.to_string())
                .collect(),
        },
    )?;
    let hello_response: HelloNegotiateResponse =
        deploy::response_payload(deploy::request_response(session, &hello_request).await?)?;
    ensure_hello_protocol_compatibility(&hello_response)?;

    Ok(HelloSummary {
        server_version: hello_response.server_version,
        features: hello_response.features,
        limits: hello_response.limits,
    })
}

pub(crate) fn ensure_hello_protocol_compatibility(
    response: &HelloNegotiateResponse,
) -> anyhow::Result<()> {
    if !response.accepted {
        return Err(anyhow!("{}", hello_rejection_message(response)));
    }

    ensure_server_protocol_version_supported(response)
}

fn hello_rejection_message(response: &HelloNegotiateResponse) -> String {
    if let Some(announcement) = response.compatibility_announcement.as_deref()
        && !announcement.trim().is_empty()
    {
        return announcement.to_string();
    }

    format!(
        "hello.negotiate was rejected by server (server_protocol_version={}, supported_protocol_version_range={})",
        response.server_protocol_version, response.supported_protocol_version_range
    )
}

fn ensure_server_protocol_version_supported(
    response: &HelloNegotiateResponse,
) -> anyhow::Result<()> {
    let supported_range = VersionReq::parse(SUPPORTED_PROTOCOL_VERSION_RANGE).map_err(|err| {
        anyhow!(
            "invalid client supported protocol range '{}': {err}",
            SUPPORTED_PROTOCOL_VERSION_RANGE
        )
    })?;
    let server_protocol_version =
        Version::parse(&response.server_protocol_version).map_err(|err| {
            anyhow!(
                "server_protocol_version '{}' is not valid semver: {err}",
                response.server_protocol_version
            )
        })?;

    if supported_range.matches(&server_protocol_version) {
        return Ok(());
    }

    Err(anyhow!(
        "server protocol version '{}' is not supported by this client (client supports '{}')",
        response.server_protocol_version,
        SUPPORTED_PROTOCOL_VERSION_RANGE
    ))
}

fn absolute_project_path(project_root: &Path) -> String {
    if project_root.is_absolute() {
        return project_root.display().to_string();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(project_root).display().to_string(),
        Err(_) => project_root.display().to_string(),
    }
}

pub(crate) fn format_local_context_line(
    project_root: &Path,
    service: &str,
    target_name: &str,
    remote: &str,
    server_name: Option<&str>,
) -> String {
    let normalized_server_name = server_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("-");
    format!(
        "cli={} project={} service={} target={} remote={} server_name={}",
        env!("CARGO_PKG_VERSION"),
        absolute_project_path(project_root),
        service,
        target_name,
        remote,
        normalized_server_name
    )
}

pub(crate) fn format_peer_context_line(
    authority: &str,
    resolved: &str,
    hello: &HelloSummary,
) -> String {
    let chunk_size = hello
        .limits
        .get("chunk_size")
        .map(String::as_str)
        .unwrap_or("-");
    let max_inflight = hello
        .limits
        .get("max_inflight_chunks")
        .map(String::as_str)
        .unwrap_or("-");
    let deploy_stream_timeout_secs = hello
        .limits
        .get("deploy_stream_timeout_secs")
        .map(String::as_str)
        .unwrap_or("-");
    format!(
        "authority={} resolved={} server_version={} limit_chunk_size={} limit_max_inflight_chunks={} limit_deploy_stream_timeout_secs={}",
        authority,
        resolved,
        hello.server_version,
        chunk_size,
        max_inflight,
        deploy_stream_timeout_secs
    )
}

pub(crate) fn handle_terminal_event(
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
        if event.event_type == CommandEventType::Progress
            && let Some(stage) = event.stage.as_deref()
        {
            ui::command_stage(command_name, stage, "remote progress");
        }
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
    use super::*;

    fn sample_hello_response() -> HelloNegotiateResponse {
        HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod/0.1.0".to_string(),
            server_protocol_version: "0.1.0".to_string(),
            supported_protocol_version_range: ">=0.1.0,<0.2.0".to_string(),
            compatibility_announcement: None,
            features: vec!["hello.negotiate".to_string()],
            limits: BTreeMap::new(),
        }
    }

    #[test]
    fn ensure_hello_protocol_compatibility_accepts_supported_server_version() {
        let response = sample_hello_response();
        assert!(ensure_hello_protocol_compatibility(&response).is_ok());
    }

    #[test]
    fn ensure_hello_protocol_compatibility_prefers_server_announcement() {
        let mut response = sample_hello_response();
        response.accepted = false;
        response.compatibility_announcement = Some("upgrade protocol".to_string());
        let err = ensure_hello_protocol_compatibility(&response)
            .expect_err("rejected response should fail");
        assert!(err.to_string().contains("upgrade protocol"));
    }

    #[test]
    fn ensure_hello_protocol_compatibility_rejects_unsupported_server_version() {
        let mut response = sample_hello_response();
        response.server_protocol_version = "0.2.0".to_string();
        let err = ensure_hello_protocol_compatibility(&response)
            .expect_err("unsupported server protocol should fail");
        assert!(err.to_string().contains("not supported"));
    }

    #[test]
    fn format_local_context_line_contains_required_keys_and_placeholder() {
        let line = format_local_context_line(
            Path::new("/tmp/imago"),
            "<all-running>",
            "default",
            "127.0.0.1:4443",
            None,
        );
        assert_eq!(
            line,
            format!(
                "cli={} project=/tmp/imago service=<all-running> target=default remote=127.0.0.1:4443 server_name=-",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn format_peer_context_line_uses_expected_limits() {
        let hello = HelloSummary {
            server_version: "imagod/0.1.0".to_string(),
            features: vec!["hello.negotiate".to_string()],
            limits: BTreeMap::from([
                ("chunk_size".to_string(), "1048576".to_string()),
                ("max_inflight_chunks".to_string(), "16".to_string()),
                ("deploy_stream_timeout_secs".to_string(), "30".to_string()),
            ]),
        };
        let line = format_peer_context_line("imagod.local:4443", "127.0.0.1:4443", &hello);
        assert_eq!(
            line,
            "authority=imagod.local:4443 resolved=127.0.0.1:4443 server_version=imagod/0.1.0 limit_chunk_size=1048576 limit_max_inflight_chunks=16 limit_deploy_stream_timeout_secs=30"
        );
    }

    #[test]
    fn format_peer_context_line_defaults_missing_limits_to_dash() {
        let hello = HelloSummary {
            server_version: "imagod/0.1.0".to_string(),
            features: vec![],
            limits: BTreeMap::new(),
        };
        let line = format_peer_context_line("imagod.local:4443", "127.0.0.1:4443", &hello);
        assert!(line.contains("limit_chunk_size=-"));
        assert!(line.contains("limit_max_inflight_chunks=-"));
        assert!(line.contains("limit_deploy_stream_timeout_secs=-"));
    }
}
