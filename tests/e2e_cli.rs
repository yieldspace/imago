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
    assert_succeeded("deploy edge", &deploy_edge.combined)?;

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
    assert_unknown_target_failure(
        "deploy unknown target",
        &deploy_unknown.combined,
        deploy_unknown.success,
    )?;

    let stop_unknown = scenario.run_service_cli(
        service.name(),
        &["stop", service.name(), "--target", "unknown"],
    )?;
    assert_unknown_target_failure(
        "stop unknown target",
        &stop_unknown.combined,
        stop_unknown.success,
    )?;

    Ok(())
}

fn assert_succeeded(label: &str, output: &str) -> TestResult {
    if output.to_ascii_lowercase().contains("succeeded") {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "{label} did not contain succeeded marker: {output}"
    ))
}

fn assert_unknown_target_failure(label: &str, output: &str, success: bool) -> TestResult {
    if success {
        return Err(anyhow::anyhow!("{label} unexpectedly succeeded: {output}"));
    }
    if output.to_ascii_lowercase().contains("unknown target")
        || output.contains("target 'unknown' is not defined in imago.toml")
    {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "{label} did not contain unknown-target specific message: {output}"
    ))
}
