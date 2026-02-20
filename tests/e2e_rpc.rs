#[path = "e2e_helper/mod.rs"]
mod e2e_helper;

use e2e_helper::certs::{generate_key_material, write_known_hosts};
use e2e_helper::cli::{CmdOutput, run_imago_cli};
use e2e_helper::{
    Cluster, TargetSpec, TestResult, WasmArtifact, wasm_file_name, wasm_path,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tempfile::Builder as TempDirBuilder;

const PRE_FAIL_MARKERS: [&str; 2] = [
    "imago:node/rpc connection failed:",
    "acme:clock/api.now failed:",
];
const SUCCESS_MARKER: &str = "acme:clock/api.now =>";

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
        &["deploy", "--target", "default"],
    )?;
    ensure_success("rpc-greeter deploy", &deploy_greeter)?;
    assert_succeeded("rpc-greeter deploy", &deploy_greeter.combined)?;

    let update_client = run_imago_cli(&workspace_root, &client_dir, &control_home, &["update"])?;
    ensure_success("cli-client update", &update_client)?;

    let deploy_client = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["deploy", "--target", "default"],
    )?;
    ensure_success("cli-client deploy", &deploy_client)?;
    assert_succeeded("cli-client deploy", &deploy_client.combined)?;

    let pre_fail_logs = wait_logs(&workspace_root, &client_dir, &control_home, 40)?;
    assert!(
        PRE_FAIL_MARKERS
            .iter()
            .any(|marker| pre_fail_logs.contains(marker)),
        "pre-cert failure marker was not found: {pre_fail_logs}"
    );

    let deploy_cert = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &[
            "bindings",
            "cert",
            "deploy",
            "--from",
            alice_authority.as_str(),
            "--to",
            bob_authority.as_str(),
        ],
    )?;
    ensure_success("bindings cert deploy", &deploy_cert)?;

    let success_logs = wait_logs_with_marker(
        &workspace_root,
        &client_dir,
        &control_home,
        SUCCESS_MARKER,
        40,
    )?;
    let returned = extract_returned_value(&success_logs)?;
    assert!(
        returned > 0,
        "rpc return value must be positive unix timestamp: {returned}"
    );

    let _ = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["stop", "cli-client", "--target", "default"],
    );
    let _ = run_imago_cli(
        &workspace_root,
        &greeter_dir,
        &control_home,
        &["stop", "rpc-greeter", "--target", "default"],
    );

    Ok(())
}

fn wait_logs(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
    retries: usize,
) -> TestResult<String> {
    for _ in 0..retries {
        let logs = run_imago_cli(
            workspace_root,
            project_dir,
            home,
            &["logs", "cli-client", "--tail", "200"],
        )?;
        if logs.success {
            return Ok(logs.combined);
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(anyhow::anyhow!("timed out while collecting cli-client logs"))
}

fn wait_logs_with_marker(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
    marker: &str,
    retries: usize,
) -> TestResult<String> {
    for _ in 0..retries {
        let logs = wait_logs(workspace_root, project_dir, home, 1)?;
        if logs.contains(marker) {
            return Ok(logs);
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(anyhow::anyhow!("timed out waiting for marker '{marker}'"))
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

fn assert_succeeded(label: &str, output: &str) -> TestResult {
    if output.to_ascii_lowercase().contains("succeeded") {
        return Ok(());
    }
    Err(anyhow::anyhow!("{label} did not contain succeeded marker: {output}"))
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
    let destination = project_dir.join("components").join(wasm_file_name(artifact));
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
    let imago_node_wit = workspace_root.join("plugins").join("imago-node").join("wit");
    let rpc_greeter_world = workspace_root.join("e2e").join("wit").join("rpc-greeter").join("world.wit");
    let body = format!(
        "name = \"cli-client\"\nmain = \"components/{}\"\ntype = \"cli\"\n\n[[dependencies]]\nname = \"imago:node\"\nversion = \"0.1.0\"\nkind = \"native\"\nwit = \"file://{}\"\n\n[capabilities]\nprivileged = false\nwasi = true\n\n[capabilities.deps]\n\"acme:clock\" = [\"*\"]\n\"imago:node\" = [\"*\"]\n\n[[bindings]]\nname = \"rpc-greeter\"\nwit = \"file://{}\"\n\n[wasi.env]\nIMAGO_RPC_ADDR = \"{}\"\n\n[target.default]\nremote = \"{}\"\nserver_name = \"{}\"\nclient_key = \"{}\"\n",
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
