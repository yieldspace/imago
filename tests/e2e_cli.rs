#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::{AppKind, CmdOutput, Scenario, TestResult, WasmArtifact};
use std::thread;
use std::time::Duration;

#[test]
#[ignore]
fn e2e_cli_deploy() -> TestResult {
    let mut scenario = Scenario::new("e2e-cli")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-cli-svc",
        AppKind::Cli,
        "default",
        WasmArtifact::CliBase,
    )?;

    let deploy_v1 = service.deploy(&scenario, "default")?;
    assert_command_completed("deploy v1", &deploy_v1)?;

    service.replace_wasm(&mut scenario, WasmArtifact::CliBase)?;
    let deploy_v2 = service.deploy(&scenario, "default")?;
    assert_command_completed("deploy v2", &deploy_v2)?;

    if let Err(err) = service.stop(&scenario, "default") {
        if !err.to_string().contains("is not running") {
            return Err(err);
        }
    }

    Ok(())
}

#[test]
#[ignore]
fn e2e_cli_deploy_and_stop_on_non_default_target() -> TestResult {
    let mut scenario = Scenario::new("e2e-clie")?;
    let _default = scenario.cluster().add_node("default")?;
    let _edge = scenario.cluster().add_node("edge")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-cli-edge-svc",
        AppKind::Cli,
        "default",
        WasmArtifact::CliBase,
    )?;

    let deploy_edge = service.deploy(&scenario, "edge")?;
    assert_command_completed("deploy edge", &deploy_edge)?;

    if let Err(err) = service.stop(&scenario, "edge") {
        if !err.to_string().contains("is not running") {
            return Err(err);
        }
    }

    Ok(())
}

#[test]
#[ignore]
fn e2e_cli_unknown_target_fails_via_run_service_cli() -> TestResult {
    let mut scenario = Scenario::new("e2e-cliu")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-cli-unknown-target-svc",
        AppKind::Cli,
        "default",
        WasmArtifact::CliBase,
    )?;

    let deploy_unknown =
        scenario.run_service_cli(service.name(), &["deploy", "--target", "unknown"])?;
    assert_unknown_target_failure("deploy unknown target", &deploy_unknown)?;

    let stop_unknown = scenario.run_service_cli(
        service.name(),
        &["stop", service.name(), "--target", "unknown"],
    )?;
    assert_unknown_target_failure("stop unknown target", &stop_unknown)?;

    Ok(())
}

#[test]
#[ignore]
fn e2e_cli_reads_dotenv_and_overrides_wasi_env() -> TestResult {
    let mut scenario = Scenario::new("e2e-clid")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-cli-dotenv-svc",
        AppKind::Cli,
        "default",
        WasmArtifact::CliBase,
    )?;

    service.append_imago_toml(
        &scenario,
        r#"
[wasi.env]
IMAGO_E2E_ENV_OVERRIDE = "from-wasi"
IMAGO_E2E_ENV_TOML_ONLY = "from-wasi-only"
"#,
    )?;
    service.write_dotenv(
        &scenario,
        "IMAGO_E2E_ENV_OVERRIDE=from-dotenv\nIMAGO_E2E_ENV_ONLY=from-dotenv-only\n",
    )?;

    let deploy = service.deploy(&scenario, "default")?;
    assert_command_completed("dotenv deploy", &deploy)?;

    let logs = wait_logs_with_markers(
        &service,
        &scenario,
        "default",
        200,
        &[
            "IMAGO_E2E_ENV_OVERRIDE=from-dotenv",
            "IMAGO_E2E_ENV_ONLY=from-dotenv-only",
            "IMAGO_E2E_ENV_TOML_ONLY=from-wasi-only",
        ],
        40,
    )?;

    assert!(
        !logs.contains("IMAGO_E2E_ENV_OVERRIDE=from-wasi"),
        "override key must not remain wasi value: {logs}"
    );

    if let Err(err) = service.stop(&scenario, "default") {
        if !err.to_string().contains("is not running") {
            return Err(err);
        }
    }

    Ok(())
}

fn wait_logs_with_markers(
    service: &e2e_helper::ServiceHandle,
    scenario: &Scenario,
    target: &str,
    tail: u32,
    markers: &[&str],
    retries: usize,
) -> TestResult<String> {
    let mut last_logs = String::new();

    for _ in 0..retries {
        let output = service.logs(scenario, target, tail)?;
        let logs = output.log_messages().join("\n");
        if markers.iter().all(|marker| logs.contains(marker)) {
            return Ok(logs);
        }
        last_logs = logs;
        thread::sleep(Duration::from_secs(1));
    }

    Err(anyhow::anyhow!(
        "timed out waiting for all markers [{}], last logs: {last_logs}",
        markers.join(", ")
    ))
}

fn assert_command_completed(label: &str, output: &CmdOutput) -> TestResult {
    match output.command_summary_status().as_deref() {
        Some("completed") => Ok(()),
        Some(status) => Err(anyhow::anyhow!(
            "{label} summary status was '{status}', expected 'completed': {}",
            output.combined
        )),
        None => Err(anyhow::anyhow!(
            "{label} command.summary was not found: {}",
            output.combined
        )),
    }
}

fn assert_unknown_target_failure(label: &str, output: &CmdOutput) -> TestResult {
    if output.success {
        return Err(anyhow::anyhow!(
            "{label} unexpectedly succeeded: {}",
            output.combined
        ));
    }

    let failed_by_contract =
        output.command_summary_status().as_deref() == Some("failed") || output.has_command_error();
    if !failed_by_contract {
        return Err(anyhow::anyhow!(
            "{label} did not emit failed summary/command.error: {}",
            output.combined
        ));
    }

    if output
        .combined
        .to_ascii_lowercase()
        .contains("unknown target")
        || output
            .combined
            .contains("target 'unknown' is not defined in imago.toml")
    {
        return Ok(());
    }

    Err(anyhow::anyhow!(
        "{label} did not contain unknown-target specific message: {}",
        output.combined
    ))
}
