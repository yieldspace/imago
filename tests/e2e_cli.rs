#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::{AppKind, Scenario, TestResult, WasmArtifact};

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
    assert_succeeded("deploy v1", &deploy_v1.combined)?;

    service.replace_wasm(&mut scenario, WasmArtifact::CliBase)?;
    let deploy_v2 = service.deploy(&scenario, "default")?;
    assert_succeeded("deploy v2", &deploy_v2.combined)?;

    if let Err(err) = service.stop(&scenario, "default") {
        if !err.to_string().contains("is not running") {
            return Err(err);
        }
    }

    Ok(())
}

fn assert_succeeded(label: &str, output: &str) -> TestResult {
    if output.to_ascii_lowercase().contains("succeeded") {
        return Ok(());
    }
    Err(anyhow::anyhow!("{label} did not contain succeeded marker: {output}"))
}
