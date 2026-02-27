#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::http::{http_post, parse_http_status};
use e2e_helper::{AppKind, CmdOutput, Scenario, TestResult, WasmArtifact};
use std::net::TcpListener;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
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
        AppKind::Http {
            port: http_port,
            max_body_bytes: None,
        },
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

#[test]
#[ignore]
fn e2e_http_large_body_burst_returns_busy_when_queue_budget_is_exhausted() -> TestResult {
    let mut scenario = Scenario::new("e2e-http-burst")?;
    let _default = scenario.cluster().add_node("default")?;
    scenario.cluster().start_all()?;

    let http_port = pick_free_port()?;
    let service = scenario.add_service(
        "e2e-http-burst-svc",
        AppKind::Http {
            port: http_port,
            max_body_bytes: Some(32 * 1024 * 1024),
        },
        "default",
        WasmArtifact::HttpSlow,
    )?;

    let deploy = service.deploy(&scenario, "default")?;
    assert_command_completed("http burst deploy", &deploy)?;

    let burst_count = 12usize;
    let start_barrier = Arc::new(Barrier::new(burst_count));
    let payload = Arc::new(vec![b'a'; 8 * 1024 * 1024]);
    let (result_tx, result_rx) = mpsc::channel::<TestResult<u16>>();
    let mut workers = Vec::with_capacity(burst_count);

    for _ in 0..burst_count {
        let start_barrier = start_barrier.clone();
        let payload = payload.clone();
        let result_tx = result_tx.clone();
        workers.push(thread::spawn(move || {
            start_barrier.wait();
            let status = http_post(http_port, payload.as_slice()).and_then(|response| {
                parse_http_status(&response)
                    .ok_or_else(|| anyhow::anyhow!("failed to parse status line: {response}"))
            });
            let _ = result_tx.send(status);
        }));
    }
    drop(result_tx);

    for worker in workers {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("burst worker thread panicked"))?;
    }

    let mut has_ok = false;
    let mut has_busy = false;
    let mut statuses = Vec::new();
    for status in result_rx {
        let status = status?;
        statuses.push(status);
        if status == 200 {
            has_ok = true;
        }
        if status == 503 {
            has_busy = true;
        }
    }

    assert!(
        has_ok,
        "expected at least one successful response, got statuses={statuses:?}"
    );
    assert!(
        has_busy,
        "expected at least one busy response, got statuses={statuses:?}"
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
