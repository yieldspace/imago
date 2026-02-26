use std::{
    io::{self, Write},
    path::Path,
    time::Instant,
};

use anyhow::Context;
use chrono::{DateTime, Local, Utc};
use imago_protocol::{
    MessageType, ServiceListRequest, ServiceListResponse, ServiceState as ProtocolServiceState,
    ServiceStatusEntry,
};
use uuid::Uuid;

use crate::{
    cli::PsArgs,
    commands::{
        CommandResult, build,
        command_common::{
            format_local_context_line, format_peer_context_line, negotiate_hello_with_features,
        },
        deploy,
        error_diagnostics::{format_command_error, summarize_command_failure},
        ui,
    },
};

const PS_HELLO_REQUIRED_FEATURES: [&str; 1] = ["services.list"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct PsSummary {
    target_name: String,
    services: usize,
}

pub async fn run(args: PsArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(args: PsArgs, project_root: &Path) -> CommandResult {
    run_with_project_root_and_target_override(args, project_root, None, None).await
}

pub(crate) async fn run_with_project_root_and_target_override(
    args: PsArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
    names_filter: Option<Vec<String>>,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("ps", "starting");
    match run_async_with_target_override(args, project_root, target_override, names_filter).await {
        Ok(summary) => {
            ui::command_finish("ps", true, "");
            let mut result = CommandResult::success("ps", started_at);
            result
                .meta
                .insert("target".to_string(), summary.target_name);
            result
                .meta
                .insert("services".to_string(), summary.services.to_string());
            result
        }
        Err(err) => {
            let summary_message = summarize_command_failure("ps", &err);
            let diagnostic_message = format_command_error("ps", &err);
            ui::command_finish("ps", false, &summary_message);
            CommandResult::failure("ps", started_at, diagnostic_message)
        }
    }
}

async fn run_async_with_target_override(
    args: PsArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
    names_filter: Option<Vec<String>>,
) -> anyhow::Result<PsSummary> {
    let target_name = if target_override.is_some() {
        "override".to_string()
    } else {
        args.target.clone()
    };
    ui::command_stage("ps", "load-config", "loading target configuration");
    let target = match target_override {
        Some(target) => target.clone(),
        None => build::load_target_config(&args.target, project_root)
            .context("failed to load target configuration")?,
    }
    .require_deploy_credentials()
    .context("target settings are invalid for ps")?;

    let service_context = ps_service_context(names_filter.as_deref());
    ui::command_info(
        "ps",
        &format_local_context_line(
            project_root,
            &service_context,
            &target_name,
            &target.remote,
            target.server_name.as_deref(),
        ),
    );

    ui::command_stage("ps", "connect", "connecting target");
    let connected = deploy::connect_target(&target).await?;

    let correlation_id = Uuid::new_v4();
    ui::command_stage("ps", "hello", "negotiating hello");
    let hello = negotiate_hello_with_features(
        &connected.session,
        correlation_id,
        &PS_HELLO_REQUIRED_FEATURES,
    )
    .await?;
    ui::command_info(
        "ps",
        &format_peer_context_line(
            &connected.authority,
            &connected.resolved_addr.to_string(),
            &hello,
        ),
    );

    ui::command_stage("ps", "services.list", "requesting service states");
    let response = request_services_list(&connected.session, correlation_id, names_filter).await?;
    let services = response.services.len();
    render_services(&response.services)?;
    Ok(PsSummary {
        target_name,
        services,
    })
}

fn ps_service_context(names_filter: Option<&[String]>) -> String {
    match names_filter {
        None => "<all-services>".to_string(),
        Some(names) if names.len() == 1 => names[0].clone(),
        Some(_) => "<filtered-services>".to_string(),
    }
}

async fn request_services_list(
    session: &web_transport_quinn::Session,
    correlation_id: Uuid,
    names_filter: Option<Vec<String>>,
) -> anyhow::Result<ServiceListResponse> {
    let request = deploy::request_envelope(
        MessageType::ServicesList,
        Uuid::new_v4(),
        correlation_id,
        &ServiceListRequest {
            names: names_filter,
        },
    )?;
    deploy::response_payload(deploy::request_response(session, &request).await?)
        .context("failed to decode services.list response")
}

fn service_state_text(state: ProtocolServiceState) -> &'static str {
    match state {
        ProtocolServiceState::Running => "running",
        ProtocolServiceState::Stopping => "stopping",
        ProtocolServiceState::Stopped => "stopped",
    }
}

fn render_services(services: &[ServiceStatusEntry]) -> anyhow::Result<()> {
    render_services_table(services)
}

fn render_services_table(services: &[ServiceStatusEntry]) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "NAME STATE RELEASE STARTED_AT").context("failed to write ps table header")?;
    for service in services {
        let started_at = format_service_started_at(service);
        writeln!(
            stdout,
            "{} {} {} {}",
            service.name,
            service_state_text(service.state),
            service.release_hash,
            started_at,
        )
        .context("failed to write ps table row")?;
    }
    Ok(())
}

fn format_service_started_at(service: &ServiceStatusEntry) -> String {
    if service.state == ProtocolServiceState::Stopped
        && (service.started_at.is_empty() || service.started_at == "0")
    {
        return "-".to_string();
    }

    format_started_at_local(&service.started_at)
}

fn format_started_at_local(started_at: &str) -> String {
    let unix_seconds = match started_at.parse::<i64>() {
        Ok(value) => value,
        Err(_) => return started_at.to_string(),
    };

    let utc = match DateTime::<Utc>::from_timestamp(unix_seconds, 0) {
        Some(value) => value,
        None => return started_at.to_string(),
    };

    utc.with_timezone(&Local)
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_service_context_uses_expected_labels() {
        assert_eq!(ps_service_context(None), "<all-services>");
        assert_eq!(
            ps_service_context(Some(&["svc-a".to_string()])),
            "svc-a".to_string()
        );
        assert_eq!(
            ps_service_context(Some(&["svc-a".to_string(), "svc-b".to_string()])),
            "<filtered-services>".to_string()
        );
    }

    #[test]
    fn service_state_text_uses_protocol_labels() {
        assert_eq!(service_state_text(ProtocolServiceState::Running), "running");
        assert_eq!(
            service_state_text(ProtocolServiceState::Stopping),
            "stopping"
        );
        assert_eq!(service_state_text(ProtocolServiceState::Stopped), "stopped");
    }

    #[test]
    fn ps_hello_required_features_are_fixed() {
        assert_eq!(PS_HELLO_REQUIRED_FEATURES, ["services.list"]);
    }

    #[test]
    fn format_started_at_local_converts_unix_seconds() {
        let formatted = format_started_at_local("1735732800");
        assert_ne!(formatted, "1735732800");
        assert_eq!(formatted.len(), 25);
        assert_eq!(formatted.as_bytes()[10], b'T');
        assert_eq!(formatted.as_bytes()[13], b':');
        assert_eq!(formatted.as_bytes()[16], b':');
        assert!(matches!(formatted.as_bytes()[19], b'+' | b'-'));
        assert_eq!(formatted.as_bytes()[22], b':');
    }

    #[test]
    fn format_started_at_local_falls_back_on_invalid_value() {
        assert_eq!(format_started_at_local("invalid"), "invalid");
    }

    #[test]
    fn format_service_started_at_uses_unknown_for_stopped_without_timestamp() {
        let stopped_empty = ServiceStatusEntry {
            name: "svc-a".to_string(),
            release_hash: "sha256:abc".to_string(),
            started_at: "".to_string(),
            state: ProtocolServiceState::Stopped,
        };
        let stopped_zero = ServiceStatusEntry {
            name: "svc-b".to_string(),
            release_hash: "sha256:def".to_string(),
            started_at: "0".to_string(),
            state: ProtocolServiceState::Stopped,
        };

        assert_eq!(format_service_started_at(&stopped_empty), "-");
        assert_eq!(format_service_started_at(&stopped_zero), "-");
    }
}
