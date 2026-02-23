#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::{AppKind, CmdOutput, Scenario, ServiceHandle, TestResult, WasmArtifact};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

const COMPLETED_MARKER: &str = "nanokvm-probe: completed";
const LINKER_DUPLICATE_MARKER: &str = "map entry `wasi:io/error@0.2.6` defined twice";

#[test]
#[ignore]
fn e2e_nanokvm_native_plugin_multi_import_does_not_duplicate_linker_entries() -> TestResult {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut scenario = Scenario::new_with_daemon_package("e2e-nanokvm", "nanokvm-imagod")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let service = scenario.add_service(
        "e2e-nanokvm-probe",
        AppKind::Cli,
        "default",
        WasmArtifact::NanoKvmProbe,
    )?;

    let nanokvm_wit = workspace_root
        .join("plugins")
        .join("imago-plugin-nanokvm-plugin")
        .join("wit");

    service.append_imago_toml(
        &scenario,
        &format!(
            "\n[[dependencies]]\nname = \"imago:nanokvm\"\nversion = \"0.1.0\"\nkind = \"native\"\nwit = \"file://{}\"\n\n[capabilities.deps]\n\"imago:nanokvm\" = [\"*\"]\n",
            toml_escape(nanokvm_wit.to_string_lossy().as_ref()),
        ),
    )?;

    let update = scenario.run_service_cli(service.name(), &["update"])?;
    ensure_success("nanokvm update", &update)?;

    let deploy = scenario.run_service_cli(service.name(), &["deploy", "--target", "default"])?;
    assert_not_defined_twice("nanokvm deploy", &deploy)?;
    ensure_success("nanokvm deploy", &deploy)?;
    assert_command_completed("nanokvm deploy", &deploy)?;

    let logs = wait_logs_with_marker(&service, &scenario, "default", 200, COMPLETED_MARKER, 40)?;
    assert!(
        !logs.contains(LINKER_DUPLICATE_MARKER),
        "unexpected linker duplicate marker in logs: {logs}"
    );

    let _ = service.stop(&scenario, "default");
    Ok(())
}

fn wait_logs_with_marker(
    service: &ServiceHandle,
    scenario: &Scenario,
    _target: &str,
    tail: u32,
    marker: &str,
    retries: usize,
) -> TestResult<String> {
    let tail_text = tail.to_string();
    let args = ["logs", service.name(), "--tail", tail_text.as_str()];
    let mut last_combined = String::new();

    for _ in 0..retries {
        let output = scenario.run_service_cli(service.name(), &args)?;
        if output.success {
            let logs = output.log_messages().join("\n");
            if logs.contains(marker) {
                return Ok(logs);
            }
        }
        last_combined = output.combined;
        thread::sleep(Duration::from_secs(1));
    }

    Err(anyhow::anyhow!(
        "timed out waiting for marker '{marker}', last output: {last_combined}"
    ))
}

fn ensure_success(label: &str, output: &CmdOutput) -> TestResult {
    if output.success {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "{label} failed: status={}, stdout={}, stderr={}",
        output.status,
        output.stdout,
        output.stderr
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

fn assert_not_defined_twice(label: &str, output: &CmdOutput) -> TestResult {
    if output.combined.contains(LINKER_DUPLICATE_MARKER)
        || (output.combined.contains("runtime.native_plugin")
            && output.combined.contains("defined twice"))
    {
        return Err(anyhow::anyhow!(
            "{label} hit linker duplicate regression: {}",
            output.combined
        ));
    }
    Ok(())
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
