use std::path::Path;

use anyhow::anyhow;
use imago_protocol::{
    CommandEvent, CommandEventType, CommandStartResponse, HelloNegotiateRequest,
    HelloNegotiateResponse, MessageType,
};
use uuid::Uuid;

use crate::commands::{build, deploy};

const HELLO_REQUIRED_FEATURES: [&str; 2] = ["command.start", "command.event"];

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
