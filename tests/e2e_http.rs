#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::{AppKind, CmdOutput, Scenario, TestResult, WasmArtifact};
use std::net::TcpListener;
use std::time::Duration;

#[test]
#[ignore]
fn e2e_http_deploy_and_respond() -> TestResult {
    let mut scenario = Scenario::new("e2e-http")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let http_port = pick_free_port()?;
    let service = scenario.add_service(
        "e2e-http-svc",
        AppKind::Http { port: http_port },
        "default",
        WasmArtifact::Http,
    )?;

    let deploy_v1 = service.deploy(&scenario, "default")?;
    assert_command_completed("http deploy v1", &deploy_v1)?;

    let response_v1 = scenario.wait_http_response(http_port, Duration::from_secs(20))?;
    assert!(
        response_v1.contains("\r\n\r\n"),
        "http v1 response body section was not present: {response_v1}"
    );

    service.replace_wasm(&mut scenario, WasmArtifact::Http)?;
    let deploy_v2 = service.deploy(&scenario, "default")?;
    assert_command_completed("http deploy v2", &deploy_v2)?;

    let response_v2 = scenario.wait_http_response(http_port, Duration::from_secs(20))?;
    assert!(
        response_v2.contains("\r\n\r\n"),
        "http v2 response body section was not present: {response_v2}"
    );

    let _ = service.stop(&scenario, "default");
    Ok(())
}

fn pick_free_port() -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
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
