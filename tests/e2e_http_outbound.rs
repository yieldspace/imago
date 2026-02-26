#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::wait::poll_until;
use e2e_helper::{AppKind, CmdOutput, Scenario, ServiceHandle, TestResult, WasmArtifact};
use std::time::Duration;

const LOG_POLL_INTERVAL: Duration = Duration::from_millis(200);
const HTTP_TARGET_AUTHORITY: &str = "127.0.0.2:18080";
const RESULT_ALLOWED: &str = "IMAGO_E2E_HTTP_OUTBOUND_RESULT=allowed";
const RESULT_DENIED: &str = "IMAGO_E2E_HTTP_OUTBOUND_RESULT=denied";

#[test]
#[ignore]
fn e2e_http_outbound_default_deny_non_allowlisted_authority() -> TestResult {
    let mut scenario = Scenario::new("e2e-hod")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-http-outbound-deny-svc",
        AppKind::Cli,
        "default",
        WasmArtifact::HttpOutboundCli,
    )?;

    service.append_imago_toml(
        &scenario,
        &format!(
            r#"
[resources.env]
IMAGO_E2E_HTTP_TARGET_AUTHORITY = "{HTTP_TARGET_AUTHORITY}"
"#
        ),
    )?;

    let deploy = service.deploy(&scenario, "default")?;
    assert_command_completed("http outbound default deny deploy", &deploy)?;

    let logs = wait_logs_with_markers(
        &service,
        &scenario,
        "default",
        200,
        &[RESULT_DENIED],
        Duration::from_secs(40),
    )?;
    assert!(
        !logs.contains(RESULT_ALLOWED),
        "deny case unexpectedly logged allowed marker: {logs}"
    );

    let _ = service.stop(&scenario, "default");
    Ok(())
}

#[test]
#[ignore]
fn e2e_http_outbound_allows_authority_when_explicit_cidr_configured() -> TestResult {
    let mut scenario = Scenario::new("e2e-hoa")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-http-outbound-allow-svc",
        AppKind::Cli,
        "default",
        WasmArtifact::HttpOutboundCli,
    )?;

    service.append_imago_toml(
        &scenario,
        &format!(
            r#"
[resources]
http_outbound = ["127.0.0.0/8"]

[resources.env]
IMAGO_E2E_HTTP_TARGET_AUTHORITY = "{HTTP_TARGET_AUTHORITY}"
"#
        ),
    )?;

    let deploy = service.deploy(&scenario, "default")?;
    assert_command_completed("http outbound explicit cidr deploy", &deploy)?;

    let logs = wait_logs_with_markers(
        &service,
        &scenario,
        "default",
        200,
        &[RESULT_ALLOWED],
        Duration::from_secs(40),
    )?;
    assert!(
        !logs.contains(RESULT_DENIED),
        "allow case unexpectedly logged denied marker: {logs}"
    );

    let _ = service.stop(&scenario, "default");
    Ok(())
}

fn wait_logs_with_markers(
    service: &ServiceHandle,
    scenario: &Scenario,
    target: &str,
    tail: u32,
    markers: &[&str],
    timeout: Duration,
) -> TestResult<String> {
    let mut last_logs = String::new();

    poll_until(
        &format!("all markers [{}] in {}", markers.join(", "), service.name()),
        timeout,
        LOG_POLL_INTERVAL,
        || {
            let output = service.logs(scenario, target, tail)?;
            if !output.success {
                return Ok(None);
            }
            let logs = output.log_messages().join("\n");
            if markers.iter().all(|marker| logs.contains(marker)) {
                return Ok(Some(logs));
            }
            last_logs = logs;
            Ok(None)
        },
    )
    .map_err(|err| anyhow::anyhow!("{err}; last logs: {last_logs}"))
}

fn assert_command_completed(label: &str, output: &CmdOutput) -> TestResult {
    match output.command_summary_status().as_deref() {
        Some("completed") => Ok(()),
        Some(status) => Err(anyhow::anyhow!(
            "{label} summary status was '{status}', expected 'completed': {}",
            output.combined
        )),
        None => Err(anyhow::anyhow!(
            "{label} completion marker was not found: {}",
            output.combined
        )),
    }
}
