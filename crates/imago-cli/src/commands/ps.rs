use std::{
    io::{self, Write},
    path::Path,
    time::Instant,
};

use anyhow::Context;
use imago_protocol::{
    MessageType, ServiceListRequest, ServiceListResponse, ServiceState as ProtocolServiceState,
    ServiceStatusEntry,
};
use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

#[cfg(unix)]
use std::ptr;

use crate::{
    cli::PsArgs,
    commands::{
        CommandResult, build,
        command_common::{
            format_local_context_line, format_peer_context_line, negotiate_hello_with_features,
        },
        deploy,
        error_diagnostics::format_command_error,
        ui,
    },
};

const PS_HELLO_REQUIRED_FEATURES: [&str; 1] = ["services.list"];

#[derive(Debug, Serialize)]
struct JsonServiceStateLine<'a> {
    #[serde(rename = "type")]
    line_type: &'static str,
    name: &'a str,
    state: &'a str,
    release: &'a str,
    started_at: &'a str,
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
        Ok(()) => {
            ui::command_finish("ps", true, "completed");
            CommandResult::success("ps", started_at).without_json_summary()
        }
        Err(err) => {
            let summary_message = err.to_string();
            let diagnostic_message = format_command_error("ps", &err);
            ui::command_finish("ps", false, &summary_message);
            let mut result = CommandResult::failure("ps", started_at, diagnostic_message.clone())
                .without_json_summary();
            if ui::current_mode() == ui::UiMode::Json {
                ui::emit_command_error_json("ps", &diagnostic_message, "ps", "E_UNKNOWN");
                result.stderr = None;
            }
            result
        }
    }
}

async fn run_async_with_target_override(
    args: PsArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
    names_filter: Option<Vec<String>>,
) -> anyhow::Result<()> {
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
    render_services(&response.services)
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
    match ui::current_mode() {
        ui::UiMode::Json => render_services_json_lines(services),
        ui::UiMode::Plain | ui::UiMode::Rich => render_services_table(services),
    }
}

fn render_services_table(services: &[ServiceStatusEntry]) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "NAME STATE RELEASE STARTED_AT").context("failed to write ps table header")?;
    for service in services {
        let started_at = format_started_at_local(&service.started_at);
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

fn render_services_json_lines(services: &[ServiceStatusEntry]) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    for service in services {
        let started_at = format_started_at_local(&service.started_at);
        let line = JsonServiceStateLine {
            line_type: "service.state",
            name: &service.name,
            state: service_state_text(service.state),
            release: &service.release_hash,
            started_at: &started_at,
        };
        serde_json::to_writer(&mut stdout, &line).context("failed to encode service.state line")?;
        stdout
            .write_all(b"\n")
            .context("failed to write service.state line delimiter")?;
    }
    Ok(())
}

fn format_started_at_local(started_at: &str) -> String {
    let unix_seconds = match started_at.parse::<i64>() {
        Ok(value) => value,
        Err(_) => return started_at.to_string(),
    };

    let utc = match OffsetDateTime::from_unix_timestamp(unix_seconds) {
        Ok(value) => value,
        Err(_) => return started_at.to_string(),
    };

    let local_offset = match local_offset_from_unix_seconds(unix_seconds) {
        Some(value) => value,
        None => return started_at.to_string(),
    };

    let local = utc.to_offset(local_offset);
    let offset_seconds = local.offset().whole_seconds();
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let abs_offset = offset_seconds.unsigned_abs();
    let offset_hours = abs_offset / 3600;
    let offset_minutes = (abs_offset % 3600) / 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
        local.year(),
        u8::from(local.month()),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
        sign,
        offset_hours,
        offset_minutes,
    )
}

#[cfg(unix)]
fn local_offset_from_unix_seconds(unix_seconds: i64) -> Option<UtcOffset> {
    let timestamp = unix_seconds.try_into().ok()?;
    let mut local_tm = LocalTm::default();
    let local_tm_ptr = unsafe {
        // SAFETY: `local_tm` is a valid out pointer and `timestamp` points to initialized data.
        localtime_r(ptr::from_ref(&timestamp), ptr::from_mut(&mut local_tm))
    };
    if local_tm_ptr.is_null() {
        return None;
    }

    let seconds = local_tm.tm_gmtoff.try_into().ok()?;
    UtcOffset::from_whole_seconds(seconds).ok()
}

#[cfg(not(unix))]
fn local_offset_from_unix_seconds(_unix_seconds: i64) -> Option<UtcOffset> {
    None
}

#[cfg(unix)]
#[allow(non_camel_case_types)]
type c_time_t = std::ffi::c_long;

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct LocalTm {
    tm_sec: std::ffi::c_int,
    tm_min: std::ffi::c_int,
    tm_hour: std::ffi::c_int,
    tm_mday: std::ffi::c_int,
    tm_mon: std::ffi::c_int,
    tm_year: std::ffi::c_int,
    tm_wday: std::ffi::c_int,
    tm_yday: std::ffi::c_int,
    tm_isdst: std::ffi::c_int,
    tm_gmtoff: std::ffi::c_long,
    tm_zone: *const std::ffi::c_char,
}

#[cfg(unix)]
unsafe extern "C" {
    fn localtime_r(timep: *const c_time_t, result: *mut LocalTm) -> *mut LocalTm;
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
    fn json_service_state_line_contains_required_fields() {
        let service = ServiceStatusEntry {
            name: "svc-a".to_string(),
            release_hash: "sha256:abc".to_string(),
            started_at: "2026-02-21T10:00:00Z".to_string(),
            state: ProtocolServiceState::Running,
        };

        let line = JsonServiceStateLine {
            line_type: "service.state",
            name: &service.name,
            state: service_state_text(service.state),
            release: &service.release_hash,
            started_at: &service.started_at,
        };
        let value = serde_json::to_value(line).expect("line should serialize");
        assert_eq!(value["type"], "service.state");
        assert_eq!(value["name"], "svc-a");
        assert_eq!(value["state"], "running");
        assert_eq!(value["release"], "sha256:abc");
        assert_eq!(value["started_at"], "2026-02-21T10:00:00Z");
    }

    #[test]
    fn ps_hello_required_features_are_fixed() {
        assert_eq!(PS_HELLO_REQUIRED_FEATURES, ["services.list"]);
    }

    #[cfg(unix)]
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
}
