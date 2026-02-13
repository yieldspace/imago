use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use openssh::{KnownHosts, SessionBuilder, Stdio};
use toml::Value as TomlValue;
use uuid::Uuid;

use crate::{
    cli::ServiceInstallArgs,
    commands::{CommandResult, build},
};

const COMPATIBILITY_DATE: &str = "2026-02-10";
const REMOTE_DIR: &str = "/tmp/imago";

#[derive(Debug, Clone)]
struct SshTargetConfig {
    ssh_host: String,
    ssh_port: u16,
    ssh_user: String,
    ssh_key: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct DaemonTlsConfig {
    ca_cert: PathBuf,
    server_cert: PathBuf,
    server_key: PathBuf,
}

struct InstallConfig {
    ssh: SshTargetConfig,
    tls: DaemonTlsConfig,
    daemon_path: PathBuf,
    remote: String,
}

pub fn run(args: ServiceInstallArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(
    args: ServiceInstallArgs,
    project_root: &Path,
) -> CommandResult {
    match run_inner(args, project_root) {
        Ok(()) => CommandResult {
            exit_code: 0,
            stderr: None,
        },
        Err(err) => CommandResult {
            exit_code: 2,
            stderr: Some(err.to_string()),
        },
    }
}

fn run_inner(args: ServiceInstallArgs, project_root: &Path) -> anyhow::Result<()> {
    let root = build::load_resolved_toml(project_root, args.env.as_deref())?;
    let target_name = &args.target;

    let targets = root
        .get("target")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("imago.toml missing required key: target"))?;
    let target_table = targets
        .get(target_name.as_str())
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("target '{}' is not defined in imago.toml", target_name))?;

    let remote = target_table
        .get("remote")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("target '{}' is missing required key: remote", target_name))?
        .to_string();

    let ssh = parse_ssh_target_config(&root, target_name, &remote)?;
    let tls = parse_daemon_tls_config(&root, target_name, project_root)?;
    let daemon_path = resolve_daemon_path(
        args.daemon_path.as_deref(),
        &root,
        target_name,
        project_root,
    )?;

    let config = InstallConfig {
        ssh,
        tls,
        daemon_path,
        remote,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;
    runtime.block_on(install_daemon(config))
}

fn parse_ssh_target_config(
    root: &toml::Table,
    target_name: &str,
    remote: &str,
) -> anyhow::Result<SshTargetConfig> {
    let targets = root
        .get("target")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("imago.toml missing required key: target"))?;
    let target_table = targets
        .get(target_name)
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("target '{}' is not defined in imago.toml", target_name))?;

    let ssh_host = match build::optional_string(target_table, "ssh_host")? {
        Some(host) => host,
        None => parse_host_from_remote(remote)?,
    };

    let ssh_port = match target_table.get("ssh_port") {
        Some(value) => value
            .as_integer()
            .ok_or_else(|| anyhow!("target key 'ssh_port' must be an integer"))?
            as u16,
        None => 22,
    };

    let ssh_user = build::optional_string(target_table, "ssh_user")?
        .ok_or_else(|| anyhow!("target '{}' is missing required key: ssh_user", target_name))?;

    let ssh_key = build::optional_string(target_table, "ssh_key")?.map(PathBuf::from);

    Ok(SshTargetConfig {
        ssh_host,
        ssh_port,
        ssh_user,
        ssh_key,
    })
}

fn parse_daemon_tls_config(
    root: &toml::Table,
    target_name: &str,
    project_root: &Path,
) -> anyhow::Result<DaemonTlsConfig> {
    let targets = root
        .get("target")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("imago.toml missing required key: target"))?;
    let target_table = targets
        .get(target_name)
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("target '{}' is not defined in imago.toml", target_name))?;

    let ca_cert = build::optional_target_cert_path(target_table, "ca_cert", project_root)?
        .ok_or_else(|| anyhow!("target '{}' is missing required key: ca_cert", target_name))?;
    let ca_dir = ca_cert.parent().unwrap_or(Path::new(".")).to_path_buf();
    let server_cert = build::optional_target_cert_path(target_table, "server_cert", project_root)?
        .unwrap_or_else(|| ca_dir.join("server.crt"));
    let server_key = build::optional_target_cert_path(target_table, "server_key", project_root)?
        .unwrap_or_else(|| ca_dir.join("server.key"));

    Ok(DaemonTlsConfig {
        ca_cert,
        server_cert,
        server_key,
    })
}

fn resolve_daemon_path(
    cli_override: Option<&Path>,
    root: &toml::Table,
    target_name: &str,
    project_root: &Path,
) -> anyhow::Result<PathBuf> {
    if let Some(path) = cli_override {
        return Ok(path.to_path_buf());
    }

    let config_path = root
        .get("target")
        .and_then(TomlValue::as_table)
        .and_then(|targets| targets.get(target_name))
        .and_then(TomlValue::as_table)
        .and_then(|t| build::optional_string(t, "daemon_path").ok().flatten())
        .map(PathBuf::from);

    if let Some(path) = config_path {
        return Ok(path);
    }

    let release_path = project_root.join("target/release/imagod");
    if release_path.exists() {
        return Ok(release_path);
    }

    let debug_path = project_root.join("target/debug/imagod");
    if debug_path.exists() {
        return Ok(debug_path);
    }

    Err(anyhow!(
        "imagod binary not found at {} or {}. Build the daemon first or use --daemon-path.",
        release_path.display(),
        debug_path.display()
    ))
}

fn parse_host_from_remote(remote: &str) -> anyhow::Result<String> {
    let url_str = if remote.contains("://") {
        remote.to_string()
    } else if let Ok(addr) = remote.parse::<std::net::SocketAddr>() {
        format!(
            "https://{}:{}/",
            format_host_for_url(&addr.ip().to_string()),
            addr.port()
        )
    } else if let Ok(ip) = remote.parse::<std::net::IpAddr>() {
        format!("https://{}:4443/", format_host_for_url(&ip.to_string()))
    } else {
        format!("https://{remote}")
    };

    let url = url::Url::parse(&url_str).context("failed to parse remote URL")?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("could not extract host from remote URL"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();

    Ok(host)
}

fn format_host_for_url(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn generate_imagod_toml(remote: &str) -> String {
    let listen_port = extract_port_from_remote(remote).unwrap_or(4443);

    format!(
        r#"listen_addr = "[::]:{listen_port}"
storage_root = "{REMOTE_DIR}/data"
server_version = "imagod/0.1.0"
compatibility_date = "{COMPATIBILITY_DATE}"

[tls]
server_cert = "{REMOTE_DIR}/certs/server.crt"
server_key = "{REMOTE_DIR}/certs/server.key"
client_ca_cert = "{REMOTE_DIR}/certs/ca.crt"

[runtime]
chunk_size = 1048576
max_inflight_chunks = 16
upload_session_ttl_secs = 900
stop_grace_timeout_secs = 30
epoch_tick_interval_ms = 50
"#,
    )
}

fn extract_port_from_remote(remote: &str) -> Option<u16> {
    if let Ok(addr) = remote.parse::<std::net::SocketAddr>() {
        return Some(addr.port());
    }
    if remote.contains("://")
        && let Ok(url) = url::Url::parse(remote)
    {
        return url.port();
    }
    let parts: Vec<&str> = remote.rsplitn(2, ':').collect();
    if parts.len() == 2
        && let Ok(port) = parts[0].parse::<u16>()
    {
        return Some(port);
    }
    None
}

async fn install_daemon(config: InstallConfig) -> anyhow::Result<()> {
    tracing::info!(
        user = %config.ssh.ssh_user,
        host = %config.ssh.ssh_host,
        port = config.ssh.ssh_port,
        "deploying daemon via SSH"
    );

    if !config.daemon_path.exists() {
        return Err(anyhow!(
            "daemon binary not found at {}",
            config.daemon_path.display()
        ));
    }

    tracing::debug!(path = %config.daemon_path.display(), "using daemon binary");

    let session = connect_ssh(&config.ssh).await?;
    tracing::debug!("SSH connection established");

    let config_toml = generate_imagod_toml(&config.remote);
    let bundle_path =
        build_daemon_bundle(&config.daemon_path, &config.tls, config_toml.as_bytes())?;

    let remote_daemon_path = format!("{REMOTE_DIR}/imagod");
    let config_path = format!("{REMOTE_DIR}/imagod.toml");

    stop_remote_daemon(&session, &remote_daemon_path).await;

    upload_and_extract_bundle(&session, &bundle_path, REMOTE_DIR).await?;
    let _ = std::fs::remove_file(&bundle_path);

    ssh_run(&session, &["chmod", "+x", &remote_daemon_path])
        .await
        .context("failed to set execute permission")?;
    ssh_run(&session, &["mkdir", "-p", &format!("{REMOTE_DIR}/data")])
        .await
        .context("failed to create storage directory")?;

    tracing::info!("upload complete");

    let pid = start_remote_daemon(&session, &remote_daemon_path, &config_path).await?;
    tracing::info!(pid = %pid, "daemon started");

    verify_daemon_running(&session, &pid).await?;

    if let Err(err) = session.close().await {
        tracing::debug!(error = %err, "SSH session close failed (non-fatal)");
    }

    Ok(())
}

async fn connect_ssh(ssh_config: &SshTargetConfig) -> anyhow::Result<openssh::Session> {
    let mut builder = SessionBuilder::default();
    builder.known_hosts_check(KnownHosts::Accept);
    builder.port(ssh_config.ssh_port);
    builder.user(ssh_config.ssh_user.clone());

    if let Some(ssh_key) = &ssh_config.ssh_key {
        builder.keyfile(ssh_key);
    }

    builder
        .connect(&ssh_config.ssh_host)
        .await
        .context("failed to establish SSH connection")
}

fn build_daemon_bundle(
    daemon_path: &Path,
    tls: &DaemonTlsConfig,
    config_toml: &[u8],
) -> anyhow::Result<PathBuf> {
    let bundle_path = std::env::temp_dir().join(format!("imago-bundle-{}.tar", Uuid::new_v4()));
    let bundle_file =
        std::fs::File::create(&bundle_path).context("failed to create bundle file")?;

    let mut builder = tar::Builder::new(bundle_file);

    add_file_to_tar(&mut builder, daemon_path, "imagod")?;
    add_file_to_tar(&mut builder, &tls.ca_cert, "certs/ca.crt")?;
    add_file_to_tar(&mut builder, &tls.server_cert, "certs/server.crt")?;
    add_file_to_tar(&mut builder, &tls.server_key, "certs/server.key")?;

    let mut header = tar::Header::new_gnu();
    header.set_size(config_toml.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "imagod.toml", config_toml)
        .context("failed to add imagod.toml to bundle")?;

    builder.finish()?;
    Ok(bundle_path)
}

fn add_file_to_tar<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    source: &Path,
    entry_name: &str,
) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(source)
        .with_context(|| format!("failed to open file for bundle: {}", source.display()))?;
    builder
        .append_file(entry_name, &mut file)
        .with_context(|| format!("failed to append tar entry: {entry_name}"))?;
    Ok(())
}

async fn ssh_run(session: &openssh::Session, args: &[&str]) -> anyhow::Result<()> {
    let output = if args.len() == 1 {
        session.command(args[0]).output().await
    } else {
        let mut cmd = session.command(args[0]);
        for arg in &args[1..] {
            cmd.arg(*arg);
        }
        cmd.output().await
    }
    .context("ssh command execution failed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("{}", stderr.trim_end()));
    }
    Ok(())
}

async fn stop_remote_daemon(session: &openssh::Session, remote_daemon_path: &str) {
    tracing::debug!("stopping existing daemon (if running)");
    let _ = session
        .command("pkill")
        .arg("-f")
        .arg(remote_daemon_path)
        .status()
        .await;

    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
}

async fn upload_and_extract_bundle(
    session: &openssh::Session,
    bundle_path: &Path,
    remote_dir: &str,
) -> anyhow::Result<()> {
    tracing::info!(remote_dir, "uploading and extracting bundle");

    ssh_run(session, &["mkdir", "-p", remote_dir])
        .await
        .context("failed to create remote directory")?;

    let bundle_file = std::fs::File::open(bundle_path).context("failed to open bundle")?;

    let output = session
        .command("tar")
        .arg("xf")
        .arg("-")
        .arg("-C")
        .arg(remote_dir)
        .stdin(Stdio::from(bundle_file))
        .output()
        .await
        .context("failed to extract bundle on remote")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("failed to extract bundle: {}", stderr));
    }

    Ok(())
}

async fn start_remote_daemon(
    session: &openssh::Session,
    remote_daemon_path: &str,
    config_path: &str,
) -> anyhow::Result<String> {
    tracing::info!("starting daemon");

    let start_output = session
        .command("sh")
        .arg("-c")
        .arg(format!(
            "nohup {} --config {} > {}/imagod.log 2>&1 & echo $!",
            remote_daemon_path, config_path, REMOTE_DIR
        ))
        .output()
        .await
        .context("failed to start daemon")?;

    if !start_output.status.success() {
        let stderr = String::from_utf8_lossy(&start_output.stderr);
        return Err(anyhow!("failed to start daemon: {}", stderr));
    }

    let pid = String::from_utf8_lossy(&start_output.stdout)
        .trim()
        .to_string();
    Ok(pid)
}

async fn verify_daemon_running(session: &openssh::Session, pid: &str) -> anyhow::Result<()> {
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let check = session
        .command("kill")
        .arg("-0")
        .arg(pid)
        .status()
        .await
        .context("failed to check daemon process")?;

    if !check.success() {
        let log = session
            .command("tail")
            .arg("-20")
            .arg(format!("{REMOTE_DIR}/imagod.log"))
            .output()
            .await
            .ok();
        let log_text = log
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        return Err(anyhow!(
            "daemon exited shortly after start. Log:\n{}",
            log_text
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-cli-service-tests-{}-{}-{}",
            test_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn write_imago_toml(root: &Path, body: &str) {
        write_file(&root.join("imago.toml"), body.as_bytes());
    }

    fn sample_toml_root(body: &str) -> toml::Table {
        let parsed: TomlValue = toml::from_str(body).expect("toml should parse");
        parsed.as_table().cloned().expect("root should be a table")
    }

    #[test]
    fn parse_ssh_target_config_extracts_all_fields() {
        let root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
ssh_host = "10.0.0.1"
ssh_port = 2222
ssh_user = "deploy"
ssh_key = "~/.ssh/id_ed25519"
"#,
        );

        let config = parse_ssh_target_config(&root, "default", "192.168.1.100:4443")
            .expect("config should parse");

        assert_eq!(config.ssh_host, "10.0.0.1");
        assert_eq!(config.ssh_port, 2222);
        assert_eq!(config.ssh_user, "deploy");
        assert_eq!(config.ssh_key, Some(PathBuf::from("~/.ssh/id_ed25519")));
    }

    #[test]
    fn parse_ssh_target_config_defaults_host_from_remote() {
        let root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
ssh_user = "deploy"
"#,
        );

        let config = parse_ssh_target_config(&root, "default", "192.168.1.100:4443")
            .expect("config should parse");

        assert_eq!(config.ssh_host, "192.168.1.100");
    }

    #[test]
    fn parse_ssh_target_config_defaults_port_to_22() {
        let root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
ssh_user = "deploy"
"#,
        );

        let config = parse_ssh_target_config(&root, "default", "192.168.1.100:4443")
            .expect("config should parse");

        assert_eq!(config.ssh_port, 22);
    }

    #[test]
    fn parse_ssh_target_config_requires_ssh_user() {
        let root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
"#,
        );

        let err = parse_ssh_target_config(&root, "default", "192.168.1.100:4443")
            .expect_err("missing ssh_user should fail");

        assert!(err.to_string().contains("ssh_user"));
    }

    #[test]
    fn parse_daemon_tls_config_requires_ca_cert() {
        let root = new_temp_dir("tls-config-missing-ca");

        let toml_root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
"#,
        );

        let err = parse_daemon_tls_config(&toml_root, "default", &root)
            .expect_err("missing ca_cert should fail");

        assert!(err.to_string().contains("ca_cert"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parse_daemon_tls_config_defaults_server_cert_from_ca_dir() {
        let root = new_temp_dir("tls-config-defaults");

        let toml_root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
ca_cert = "certs/ca.crt"
"#,
        );

        let config = parse_daemon_tls_config(&toml_root, "default", &root)
            .expect("config should parse with defaults");

        assert_eq!(config.server_cert, root.join("certs/server.crt"));
        assert_eq!(config.server_key, root.join("certs/server.key"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_daemon_path_prefers_cli_override() {
        let root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
daemon_path = "target/release/imagod"
"#,
        );

        let project_root = new_temp_dir("daemon-path-cli");
        let cli_path = Path::new("/usr/local/bin/imagod");

        let result = resolve_daemon_path(Some(cli_path), &root, "default", &project_root)
            .expect("cli override should be used");

        assert_eq!(result, PathBuf::from("/usr/local/bin/imagod"));

        let _ = fs::remove_dir_all(project_root);
    }

    #[test]
    fn resolve_daemon_path_uses_config_value() {
        let root = sample_toml_root(
            r#"
[target.default]
remote = "192.168.1.100:4443"
daemon_path = "my-custom/imagod"
"#,
        );

        let project_root = new_temp_dir("daemon-path-config");

        let result = resolve_daemon_path(None, &root, "default", &project_root)
            .expect("config path should be used");

        assert_eq!(result, PathBuf::from("my-custom/imagod"));

        let _ = fs::remove_dir_all(project_root);
    }

    #[test]
    fn generate_imagod_toml_contains_expected_sections() {
        let toml_content = generate_imagod_toml("192.168.1.100:4443");

        assert!(toml_content.contains("listen_addr"));
        assert!(toml_content.contains("4443"));
        assert!(toml_content.contains("[tls]"));
        assert!(toml_content.contains("server_cert"));
        assert!(toml_content.contains("server_key"));
        assert!(toml_content.contains("client_ca_cert"));
        assert!(toml_content.contains("[runtime]"));
        assert!(toml_content.contains(COMPATIBILITY_DATE));
        assert!(toml_content.contains(REMOTE_DIR));
    }

    #[test]
    fn run_with_project_root_returns_error_without_imago_toml() {
        let root = new_temp_dir("service-no-toml");

        let result = run_with_project_root(
            ServiceInstallArgs {
                env: None,
                target: "default".to_string(),
                daemon_path: None,
            },
            &root,
        );

        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());

        let _ = fs::remove_dir_all(root);
    }
}
