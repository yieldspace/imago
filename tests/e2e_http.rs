#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::{AppKind, Scenario, TestResult, WasmArtifact};
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
    assert_succeeded("http deploy v1", &deploy_v1.combined)?;

    let response_v1 = scenario.wait_http_response(http_port, Duration::from_secs(20))?;
    assert!(
        response_v1.contains("\r\n\r\n"),
        "http v1 response body section was not present: {response_v1}"
    );

    service.replace_wasm(&mut scenario, WasmArtifact::Http)?;
    let deploy_v2 = service.deploy(&scenario, "default")?;
    assert_succeeded("http deploy v2", &deploy_v2.combined)?;

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

fn assert_succeeded(label: &str, output: &str) -> TestResult {
    if output.to_ascii_lowercase().contains("succeeded") {
        return Ok(());
    }
    Err(anyhow::anyhow!("{label} did not contain succeeded marker: {output}"))
}
