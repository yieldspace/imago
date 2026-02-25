#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::{AppKind, CmdOutput, Scenario, TestResult, WasmArtifact};

const FAIL_STDOUT_MARKER: &str = "IMAGO_E2E_DEPLOY_FAIL_STDOUT";
const FAIL_STDERR_MARKER: &str = "IMAGO_E2E_DEPLOY_FAIL_STDERR";
const DEPLOY_FAIL_MAX_ATTEMPTS: usize = 3;

#[test]
#[ignore]
fn e2e_cli_deploy_failure_includes_wasm_logs() -> TestResult {
    let mut scenario = Scenario::new("e2e-clif")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-cli-fail-svc",
        AppKind::Cli,
        "default",
        WasmArtifact::CliFail,
    )?;

    let mut deploy_failed = None;
    let mut last_output = None;
    for _attempt in 1..=DEPLOY_FAIL_MAX_ATTEMPTS {
        let output =
            scenario.run_service_cli(service.name(), &["deploy", "--target", "default"])?;
        if !output.success {
            deploy_failed = Some(output);
            break;
        }
        last_output = Some(output.combined);
    }
    let deploy_failed = deploy_failed.ok_or_else(|| {
        anyhow::anyhow!(
            "deploy did not fail after {DEPLOY_FAIL_MAX_ATTEMPTS} attempts: {}",
            last_output.unwrap_or_else(|| "<no output>".to_string())
        )
    })?;
    assert_deploy_failure_includes_wasm_logs("deploy fail wasm logs", &deploy_failed)?;

    Ok(())
}

fn assert_deploy_failure_includes_wasm_logs(label: &str, output: &CmdOutput) -> TestResult {
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

    let failure_message = output
        .command_summary_error()
        .unwrap_or_else(|| output.command_error_messages().join("\n"));
    if failure_message.is_empty() {
        return Err(anyhow::anyhow!(
            "{label} did not contain any failure message body: {}",
            output.combined
        ));
    }

    for expected in [
        "wasm stdout:",
        "wasm stderr:",
        FAIL_STDOUT_MARKER,
        FAIL_STDERR_MARKER,
    ] {
        if !failure_message.contains(expected) {
            return Err(anyhow::anyhow!(
                "{label} did not contain '{expected}' in failure message: {failure_message}"
            ));
        }
    }

    Ok(())
}
