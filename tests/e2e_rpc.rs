#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::certs::{generate_key_material, write_known_hosts};
use e2e_helper::cli::{CmdOutput, run_imago_cli};
use e2e_helper::wait::poll_until;
use e2e_helper::{Cluster, TargetSpec, TestResult, WasmArtifact, wasm_file_name, wasm_path};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::Builder as TempDirBuilder;

const PRE_FAIL_MARKERS: [&str; 2] = [
    "imago:node/rpc connection failed:",
    "acme:clock/api.now failed:",
];
const SUCCESS_MARKER: &str = "acme:clock/api.now =>";
const LOG_WAIT_TIMEOUT: Duration = Duration::from_secs(40);
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[test]
#[ignore]
fn e2e_rpc_two_nodes_cert_flow() -> TestResult {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp = TempDirBuilder::new().prefix("ierpc").tempdir()?;

    let control_keys = generate_key_material(&temp.path().join("control"))?;
    let control_home = temp.path().join("h");
    fs::create_dir_all(&control_home)?;

    let mut cluster = Cluster::new(
        workspace_root.clone(),
        temp.path().join("n"),
        control_keys.admin_public_hex.clone(),
    )?;
    let _alice = cluster.add_node("alice")?;
    let _bob = cluster.add_node("bob")?;
    cluster.start_all()?;
    write_known_hosts(&control_home, &cluster.known_hosts_entries())?;

    let services_root = temp.path().join("s");
    let greeter_dir = services_root.join("g");
    let client_dir = services_root.join("c");
    prepare_project_dir(&greeter_dir)?;
    prepare_project_dir(&client_dir)?;

    install_control_key(&greeter_dir, &control_keys.admin_key_path)?;
    install_control_key(&client_dir, &control_keys.admin_key_path)?;
    install_wasm(&greeter_dir, WasmArtifact::RpcCallee)?;
    install_wasm(&client_dir, WasmArtifact::RpcCaller)?;

    let bob_target = cluster.target("bob")?;
    let alice_target = cluster.target("alice")?;
    let bob_authority = cluster.authority_for("bob")?;
    let alice_authority = cluster.authority_for("alice")?;

    write_rpc_greeter_imago_toml(
        &greeter_dir,
        &bob_target,
        wasm_file_name(WasmArtifact::RpcCallee),
    )?;
    write_cli_client_imago_toml(
        &client_dir,
        &alice_target,
        &bob_authority,
        workspace_root.as_path(),
        wasm_file_name(WasmArtifact::RpcCaller),
    )?;

    let deploy_greeter = run_imago_cli(
        &workspace_root,
        &greeter_dir,
        &control_home,
        &["service", "deploy", "--target", "default", "--detach"],
    )?;
    ensure_success("rpc-greeter deploy", &deploy_greeter)?;
    assert_command_completed("rpc-greeter deploy", &deploy_greeter)?;

    let update_client = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["deps", "sync"],
    )?;
    ensure_success("cli-client update", &update_client)?;

    let deploy_client = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["service", "deploy", "--target", "default", "--detach"],
    )?;
    ensure_success("cli-client deploy", &deploy_client)?;
    assert_command_completed("cli-client deploy", &deploy_client)?;

    let _pre_fail_logs = wait_logs_with_any_marker(
        &workspace_root,
        &client_dir,
        &control_home,
        &PRE_FAIL_MARKERS,
        LOG_WAIT_TIMEOUT,
    )?;

    let invalid_to_authority = "rpc://[::1";
    let deploy_cert_partial_fail = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &[
            "trust",
            "cert",
            "replicate",
            "--from",
            alice_authority.as_str(),
            "--to",
            invalid_to_authority,
        ],
    )?;
    assert!(
        !deploy_cert_partial_fail.success,
        "bindings cert deploy (partial failure) unexpectedly succeeded: {}",
        deploy_cert_partial_fail.combined
    );
    let failed_by_contract = deploy_cert_partial_fail.command_summary_status().as_deref()
        == Some("failed")
        || deploy_cert_partial_fail.has_command_error();
    assert!(
        failed_by_contract,
        "bindings cert deploy (partial failure) did not emit failure marker: {}",
        deploy_cert_partial_fail.combined
    );

    let partial_fail_message = deploy_cert_partial_fail
        .command_summary_error()
        .unwrap_or_else(|| deploy_cert_partial_fail.command_error_messages().join("\n"));
    let has_partial_status = partial_fail_message.contains("from: ok")
        && partial_fail_message.contains("to: upload failed:");
    let has_to_authority_validation_failure =
        partial_fail_message.contains("failed to normalize --to authority:");
    assert!(
        has_partial_status || has_to_authority_validation_failure,
        "partial failure marker was not found: {partial_fail_message}"
    );
    if has_to_authority_validation_failure {
        assert!(
            partial_fail_message.contains("remote URL parse failed")
                || partial_fail_message.contains("invalid IPv6 address"),
            "authority normalization failure detail was not found: {partial_fail_message}"
        );
    }

    let deploy_cert = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &[
            "trust",
            "cert",
            "replicate",
            "--from",
            alice_authority.as_str(),
            "--to",
            bob_authority.as_str(),
        ],
    )?;
    ensure_success("bindings cert deploy", &deploy_cert)?;
    assert_command_completed("bindings cert deploy", &deploy_cert)?;

    let success_logs = wait_logs_with_marker(
        &workspace_root,
        &client_dir,
        &control_home,
        SUCCESS_MARKER,
        LOG_WAIT_TIMEOUT,
    )?;
    let returned = extract_returned_value(&success_logs)?;
    assert!(
        returned > 0,
        "rpc return value must be positive unix timestamp: {returned}"
    );

    let _greeter_logs = wait_logs_for_service(
        &workspace_root,
        &greeter_dir,
        &control_home,
        "rpc-greeter",
        LOG_WAIT_TIMEOUT,
    )?;

    let _ = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["service", "stop", "cli-client", "--target", "default"],
    );
    let _ = run_imago_cli(
        &workspace_root,
        &greeter_dir,
        &control_home,
        &["service", "stop", "rpc-greeter", "--target", "default"],
    );

    Ok(())
}

fn wait_logs(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
    timeout: Duration,
) -> TestResult<String> {
    wait_logs_for_service(workspace_root, project_dir, home, "cli-client", timeout)
}

fn wait_logs_for_service(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
    service_name: &str,
    timeout: Duration,
) -> TestResult<String> {
    poll_until(
        &format!("collecting {service_name} logs"),
        timeout,
        LOG_POLL_INTERVAL,
        || fetch_logs_once(workspace_root, project_dir, home, service_name),
    )
}

fn wait_logs_with_marker(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
    marker: &str,
    timeout: Duration,
) -> TestResult<String> {
    let mut last_logs = String::new();
    poll_until(
        &format!("marker '{marker}' in cli-client logs"),
        timeout,
        LOG_POLL_INTERVAL,
        || {
            let Some(logs) = fetch_logs_once(workspace_root, project_dir, home, "cli-client")?
            else {
                return Ok(None);
            };
            last_logs = logs.clone();
            if logs.contains(marker) {
                return Ok(Some(logs));
            }
            Ok(None)
        },
    )
    .map_err(|err| anyhow::anyhow!("{err}; last logs: {last_logs}"))
}

fn wait_logs_with_any_marker(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
    markers: &[&str],
    timeout: Duration,
) -> TestResult<String> {
    let mut last_logs = String::new();
    poll_until(
        &format!("any marker [{}] in cli-client logs", markers.join(", ")),
        timeout,
        LOG_POLL_INTERVAL,
        || {
            let Some(logs) = fetch_logs_once(workspace_root, project_dir, home, "cli-client")?
            else {
                return Ok(None);
            };
            last_logs = logs.clone();
            if markers.iter().any(|marker| logs.contains(marker)) {
                return Ok(Some(logs));
            }
            Ok(None)
        },
    )
    .map_err(|err| anyhow::anyhow!("{err}; last logs: {last_logs}"))
}

fn fetch_logs_once(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
    service_name: &str,
) -> TestResult<Option<String>> {
    let logs = run_imago_cli(
        workspace_root,
        project_dir,
        home,
        &["service", "logs", service_name, "--tail", "200"],
    )?;
    if !logs.success {
        return Ok(None);
    }
    Ok(Some(logs.log_messages().join("\n")))
}

fn extract_returned_value(logs: &str) -> TestResult<u64> {
    for line in logs.lines().rev() {
        if let Some((_head, value)) = line.split_once(SUCCESS_MARKER) {
            let parsed = value.trim().parse::<u64>()?;
            return Ok(parsed);
        }
    }
    Err(anyhow::anyhow!(
        "success marker not found in logs: {SUCCESS_MARKER}"
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
            "{label} completion marker was not found: {}",
            output.combined
        )),
    }
}

fn prepare_project_dir(project_dir: &Path) -> TestResult {
    fs::create_dir_all(project_dir.join("components"))?;
    fs::create_dir_all(project_dir.join("certs"))?;
    Ok(())
}

fn install_control_key(project_dir: &Path, control_key_path: &Path) -> TestResult {
    let cert_dir = project_dir.join("certs");
    fs::create_dir_all(&cert_dir)?;
    fs::copy(control_key_path, cert_dir.join("control.key"))?;
    Ok(())
}

fn install_wasm(project_dir: &Path, artifact: WasmArtifact) -> TestResult<PathBuf> {
    let source = wasm_path(artifact)?;
    let destination = project_dir
        .join("components")
        .join(wasm_file_name(artifact));
    fs::copy(source, &destination)?;
    Ok(destination)
}

fn write_rpc_greeter_imago_toml(
    project_dir: &Path,
    target: &TargetSpec,
    main_wasm_file: &str,
) -> TestResult {
    let body = format!(
        "name = \"rpc-greeter\"\nmain = \"components/{}\"\ntype = \"rpc\"\n\n[capabilities]\nprivileged = false\nwasi = true\n\n[target.default]\nremote = \"{}\"\nserver_name = \"{}\"\nclient_key = \"{}\"\n",
        toml_escape(main_wasm_file),
        toml_escape(&target.remote),
        toml_escape(&target.server_name),
        toml_escape(&target.client_key_rel),
    );
    fs::write(project_dir.join("imago.toml"), body)?;
    Ok(())
}

fn write_cli_client_imago_toml(
    project_dir: &Path,
    target: &TargetSpec,
    rpc_addr: &str,
    workspace_root: &Path,
    main_wasm_file: &str,
) -> TestResult {
    let imago_node_wit = workspace_root
        .join("plugins")
        .join("imago-node")
        .join("wit");
    let rpc_greeter_world = workspace_root
        .join("e2e")
        .join("wit")
        .join("rpc-greeter")
        .join("world.wit");
    let body = format!(
        "name = \"cli-client\"\nmain = \"components/{}\"\ntype = \"cli\"\n\n[[dependencies]]\nname = \"imago:node\"\nversion = \"0.1.0\"\nkind = \"native\"\nwit = \"file://{}\"\n\n[capabilities]\nprivileged = false\nwasi = true\n\n[capabilities.deps]\n\"acme:clock\" = [\"*\"]\n\"imago:node\" = [\"*\"]\n\n[[bindings]]\nname = \"rpc-greeter\"\nwit = \"file://{}\"\n\n[resources.env]\nIMAGO_RPC_ADDR = \"{}\"\n\n[target.default]\nremote = \"{}\"\nserver_name = \"{}\"\nclient_key = \"{}\"\n",
        toml_escape(main_wasm_file),
        toml_escape(imago_node_wit.to_string_lossy().as_ref()),
        toml_escape(rpc_greeter_world.to_string_lossy().as_ref()),
        toml_escape(rpc_addr),
        toml_escape(&target.remote),
        toml_escape(&target.server_name),
        toml_escape(&target.client_key_rel),
    );
    fs::write(project_dir.join("imago.toml"), body)?;
    Ok(())
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
