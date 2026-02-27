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

const SUCCESS_MARKER: &str = "acme:clock/api.now =>";
const LOG_WAIT_TIMEOUT: Duration = Duration::from_secs(40);
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[test]
#[ignore]
fn e2e_rpc_single_node_local_flow() -> TestResult {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp = TempDirBuilder::new().prefix("ierpcl").tempdir()?;

    let control_keys = generate_key_material(&temp.path().join("control"))?;
    let control_home = temp.path().join("h");
    fs::create_dir_all(&control_home)?;

    let mut cluster = Cluster::new(
        workspace_root.clone(),
        temp.path().join("n"),
        control_keys.admin_public_hex.clone(),
    )?;
    let _default = cluster.add_node("default")?;
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

    let default_target = cluster.target("default")?;

    write_rpc_greeter_imago_toml(
        &greeter_dir,
        &default_target,
        wasm_file_name(WasmArtifact::RpcCallee),
    )?;
    write_cli_client_imago_toml(
        &client_dir,
        &default_target,
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

    let deps_sync_client = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["deps", "sync"],
    )?;
    ensure_success("rpc-caller deps sync", &deps_sync_client)?;

    let deploy_client = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["service", "deploy", "--target", "default", "--detach"],
    )?;
    ensure_success("rpc-caller deploy", &deploy_client)?;
    assert_command_completed("rpc-caller deploy", &deploy_client)?;

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

    let _ = run_imago_cli(
        &workspace_root,
        &client_dir,
        &control_home,
        &["service", "stop", "rpc-caller", "--target", "default"],
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
    poll_until(
        "collecting rpc-caller logs",
        timeout,
        LOG_POLL_INTERVAL,
        || fetch_logs_once(workspace_root, project_dir, home),
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
        &format!("marker '{marker}' in rpc-caller logs"),
        timeout,
        LOG_POLL_INTERVAL,
        || {
            let Some(logs) = fetch_logs_once(workspace_root, project_dir, home)? else {
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

fn fetch_logs_once(
    workspace_root: &Path,
    project_dir: &Path,
    home: &Path,
) -> TestResult<Option<String>> {
    let logs = run_imago_cli(
        workspace_root,
        project_dir,
        home,
        &["service", "logs", "rpc-caller", "--tail", "200"],
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
        "name = \"rpc-caller\"\nmain = \"components/{}\"\ntype = \"cli\"\n\n[[dependencies]]\nname = \"imago:node\"\nversion = \"0.1.0\"\nkind = \"native\"\nwit = \"file://{}\"\n\n[capabilities]\nprivileged = false\nwasi = true\n\n[capabilities.deps]\n\"acme:clock\" = [\"*\"]\n\"imago:node\" = [\"*\"]\n\n[[bindings]]\nname = \"rpc-greeter\"\nwit = \"file://{}\"\n\n[target.default]\nremote = \"{}\"\nserver_name = \"{}\"\nclient_key = \"{}\"\n",
        toml_escape(main_wasm_file),
        toml_escape(imago_node_wit.to_string_lossy().as_ref()),
        toml_escape(rpc_greeter_world.to_string_lossy().as_ref()),
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
