#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::{AppKind, CmdOutput, Scenario, TestResult, WasmArtifact};
use std::ffi::OsString;

const FAIL_STDOUT_MARKER: &str = "IMAGO_E2E_DEPLOY_FAIL_STDOUT";
const FAIL_STDERR_MARKER: &str = "IMAGO_E2E_DEPLOY_FAIL_STDERR";
const DEPLOY_FAIL_MAX_ATTEMPTS: usize = 3;
const RUNNER_STARTUP_CONFIRM_WINDOW_ENV: &str = "IMAGOD_RUNNER_STARTUP_CONFIRM_WINDOW_MS";
const RUNNER_STARTUP_CONFIRM_WINDOW_E2E_MS: &str = "5000";

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: This test binary has a single ignored test and uses this override
        // only for spawned child processes in this test scope.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => {
                // SAFETY: Restores prior process environment value at end of this test scope.
                unsafe {
                    std::env::set_var(self.key, value);
                }
            }
            None => {
                // SAFETY: Removes test-local override set in this scope.
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}

#[test]
#[ignore]
fn e2e_cli_deploy_failure_includes_wasm_logs() -> TestResult {
    let _startup_window = ScopedEnvVar::set(
        RUNNER_STARTUP_CONFIRM_WINDOW_ENV,
        RUNNER_STARTUP_CONFIRM_WINDOW_E2E_MS,
    );

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
        let output = scenario.run_service_cli(
            service.name(),
            &["deploy", "--target", "default", "--detach"],
        )?;
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
            "{label} did not emit failure marker: {}",
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

#[test]
fn has_command_error_requires_error_marker() {
    let output = CmdOutput {
        status: "exit status: 1".to_string(),
        status_code: Some(1),
        success: false,
        stdout: String::new(),
        stderr: "deployment failed without marker".to_string(),
        combined: "deployment failed without marker".to_string(),
    };

    assert!(
        !output.has_command_error(),
        "non-empty stderr without [error] marker must not set command error contract"
    );
    assert_eq!(
        output.command_summary_error(),
        None,
        "summary error must only come from [error] markers"
    );
    assert_eq!(
        output.command_error_messages(),
        vec!["deployment failed without marker".to_string()],
        "diagnostic fallback should remain available in command_error_messages"
    );
}
