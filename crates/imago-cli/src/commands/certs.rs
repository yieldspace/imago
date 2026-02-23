use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, anyhow};
use imago_protocol::{BindingsCertUploadRequest, BindingsCertUploadResponse, MessageType};
use rcgen::{KeyPair, PKCS_ED25519};
use url::Url;
use uuid::Uuid;
use web_transport_quinn::Session;

use crate::{
    cli::{BindingsCertDeployArgs, BindingsCertUploadArgs, CertsGenerateArgs},
    commands::{
        build,
        command_common::{
            format_local_context_line, format_peer_context_line, negotiate_hello_with_features,
        },
        deploy,
        error_diagnostics::format_command_error,
        ui,
    },
};

use super::CommandResult;

const GITIGNORE_CONTENT: &str = "*\n!.gitignore\n";
const BINDINGS_CERT_UPLOAD_FEATURE: &str = "bindings.cert.upload";
const BINDINGS_CERT_UPLOAD_REQUIRED_FEATURES: [&str; 1] = [BINDINGS_CERT_UPLOAD_FEATURE];
const BINDINGS_CERT_NO_REQUIRED_FEATURES: [&str; 0] = [];
const IMAGO_DIR_NAME: &str = ".imago";
const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts";

#[derive(Debug)]
struct OutputPaths {
    client_key: PathBuf,
    gitignore: PathBuf,
}

#[derive(Debug)]
struct GenerateOutput {
    paths: OutputPaths,
    client_public_key_hex: String,
}

pub fn run_generate(args: CertsGenerateArgs) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("certs.generate", "starting");
    ui::command_stage("certs.generate", "generate", "creating key material");
    match run_generate_inner(args) {
        Ok(output) => {
            if ui::current_mode() != ui::UiMode::Json {
                println!("generated key material:");
                println!("  {}", output.paths.client_key.display());
                println!("  {}", output.paths.gitignore.display());
                println!(
                    "  client_public_key_hex={}",
                    output.client_public_key_hex.as_str()
                );
                println!("private keys are sensitive. do not commit or share them.");
            }

            ui::command_finish("certs.generate", true, "completed");
            success_generate_result(started_at, output.client_public_key_hex)
        }
        Err(err) => {
            let summary_message = err.to_string();
            let diagnostic_message = format_command_error("certs.generate", &err);
            ui::command_finish("certs.generate", false, &summary_message);
            CommandResult::failure("certs.generate", started_at, diagnostic_message)
        }
    }
}

fn success_generate_result(started_at: Instant, client_public_key_hex: String) -> CommandResult {
    let mut result = CommandResult::success("certs.generate", started_at);
    result
        .meta
        .insert("client_public_key_hex".to_string(), client_public_key_hex);
    result
}

pub async fn run_bindings_cert_upload(args: BindingsCertUploadArgs) -> CommandResult {
    run_bindings_cert_upload_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_bindings_cert_upload_with_project_root(
    args: BindingsCertUploadArgs,
    project_root: &Path,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("bindings.cert.upload", "starting");
    match run_bindings_cert_upload_async(args, project_root).await {
        Ok(()) => {
            ui::command_finish("bindings.cert.upload", true, "completed");
            CommandResult::success("bindings.cert.upload", started_at)
        }
        Err(err) => {
            let summary_message = err.to_string();
            let diagnostic_message = format_command_error("bindings.cert.upload", &err);
            ui::command_finish("bindings.cert.upload", false, &summary_message);
            CommandResult::failure("bindings.cert.upload", started_at, diagnostic_message)
        }
    }
}

pub async fn run_bindings_cert_deploy(args: BindingsCertDeployArgs) -> CommandResult {
    run_bindings_cert_deploy_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_bindings_cert_deploy_with_project_root(
    args: BindingsCertDeployArgs,
    project_root: &Path,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("bindings.cert.deploy", "starting");
    match run_bindings_cert_deploy_async(args, project_root).await {
        Ok(()) => {
            ui::command_finish("bindings.cert.deploy", true, "completed");
            CommandResult::success("bindings.cert.deploy", started_at)
        }
        Err(err) => {
            let summary_message = err.to_string();
            let diagnostic_message = format_command_error("bindings.cert.deploy", &err);
            ui::command_finish("bindings.cert.deploy", false, &summary_message);
            CommandResult::failure("bindings.cert.deploy", started_at, diagnostic_message)
        }
    }
}

async fn run_bindings_cert_upload_async(
    args: BindingsCertUploadArgs,
    project_root: &Path,
) -> anyhow::Result<()> {
    ui::command_stage(
        "bindings.cert.upload",
        "load-config",
        "loading target credentials",
    );
    let public_key_hex = normalize_ed25519_public_key_hex(&args.public_key)
        .context("invalid PUBLIC_KEY_HEX for bindings cert upload")?;
    let authority = normalize_known_hosts_authority(&args.to)
        .with_context(|| format!("failed to normalize --to authority: {}", args.to))?;
    let client_key = load_bindings_client_key(project_root)?;
    ui::command_info(
        "bindings.cert.upload",
        &format_local_context_line(
            project_root,
            "bindings.cert.upload",
            build::default_target_name(),
            &args.to,
            None,
        ),
    );

    upload_public_key_to_remote(
        "bindings.cert.upload",
        &args.to,
        &public_key_hex,
        &authority,
        &client_key,
        &BINDINGS_CERT_UPLOAD_REQUIRED_FEATURES,
    )
    .await
}

async fn run_bindings_cert_deploy_async(
    args: BindingsCertDeployArgs,
    project_root: &Path,
) -> anyhow::Result<()> {
    ui::command_stage(
        "bindings.cert.deploy",
        "load-config",
        "loading target credentials",
    );
    let client_key = load_bindings_client_key(project_root)?;
    let mut from_failures = Vec::new();
    ui::command_info(
        "bindings.cert.deploy",
        &format_local_context_line(
            project_root,
            "bindings.cert.deploy.from",
            build::default_target_name(),
            &args.from,
            None,
        ),
    );

    match connect_remote(&args.from, &client_key).await {
        Ok(connected) => {
            ui::command_stage("bindings.cert.deploy", "hello", "negotiating hello (from)");
            let correlation_id = Uuid::new_v4();
            match negotiate_bindings_cert_hello(
                &connected.session,
                correlation_id,
                &BINDINGS_CERT_NO_REQUIRED_FEATURES,
            )
            .await
            {
                Ok(hello) => {
                    ui::command_info(
                        "bindings.cert.deploy",
                        &format_peer_context_line(
                            &connected.authority,
                            &connected.resolved_addr.to_string(),
                            &hello,
                        ),
                    );
                    connected
                        .session
                        .close(0, b"bindings cert deploy from probe complete");
                }
                Err(err) => from_failures.push(format!("hello failed: {err}")),
            }
        }
        Err(err) => from_failures.push(format!("connect failed: {err}")),
    }

    let from_authority = normalize_known_hosts_authority(&args.from)
        .with_context(|| format!("failed to normalize --from authority: {}", args.from))?;
    let known_hosts_path = resolve_known_hosts_path()?;

    let from_public_key_hex = match read_known_host_public_key(&known_hosts_path, &from_authority) {
        Ok(key) => Some(key),
        Err(err) => {
            from_failures.push(format!(
                "public key lookup failed for authority '{from_authority}' in {}: {err}",
                known_hosts_path.display()
            ));
            None
        }
    };

    let to_error = if let Some(public_key_hex) = from_public_key_hex.as_deref() {
        ui::command_info(
            "bindings.cert.deploy",
            &format_local_context_line(
                project_root,
                "bindings.cert.deploy.to",
                build::default_target_name(),
                &args.to,
                None,
            ),
        );
        upload_public_key_to_remote(
            "bindings.cert.deploy",
            &args.to,
            public_key_hex,
            &from_authority,
            &client_key,
            &BINDINGS_CERT_UPLOAD_REQUIRED_FEATURES,
        )
        .await
        .err()
        .map(|err| format!("upload failed: {err}"))
    } else {
        Some("skipped because from public key is unavailable".to_string())
    };

    let from_error = if from_failures.is_empty() {
        None
    } else {
        Some(from_failures.join("; "))
    };

    if from_error.is_none() && to_error.is_none() {
        return Ok(());
    }

    Err(anyhow!(format_bindings_cert_deploy_result(
        from_error.as_deref(),
        to_error.as_deref()
    )))
}

fn load_bindings_client_key(project_root: &Path) -> anyhow::Result<PathBuf> {
    let target = build::load_target_config(build::default_target_name(), project_root)
        .context("failed to load target configuration")?
        .require_deploy_credentials()
        .context("target settings are invalid for bindings cert commands")?;
    Ok(target.client_key)
}

async fn connect_remote(
    remote: &str,
    client_key: &Path,
) -> anyhow::Result<deploy::ConnectedTargetSession> {
    let target = build::DeployTargetConfig {
        remote: remote.to_string(),
        server_name: None,
        client_key: client_key.to_path_buf(),
    };
    deploy::connect_target(&target).await
}

async fn upload_public_key_to_remote(
    command_name: &str,
    remote: &str,
    public_key_hex: &str,
    authority: &str,
    client_key: &Path,
    required_features: &[&str],
) -> anyhow::Result<()> {
    ui::command_stage(command_name, "connect", "connecting remote");
    let connected = connect_remote(remote, client_key).await?;
    let correlation_id = Uuid::new_v4();

    ui::command_stage(command_name, "hello", "negotiating hello");
    let hello =
        negotiate_bindings_cert_hello(&connected.session, correlation_id, required_features)
            .await?;
    ui::command_info(
        command_name,
        &format_peer_context_line(
            &connected.authority,
            &connected.resolved_addr.to_string(),
            &hello,
        ),
    );
    ui::command_stage(command_name, "upload", "uploading public key");
    send_bindings_cert_upload_request(
        command_name,
        &connected.session,
        correlation_id,
        public_key_hex,
        authority,
    )
    .await?;
    connected.session.close(0, b"bindings cert upload complete");

    Ok(())
}

async fn negotiate_bindings_cert_hello(
    session: &Session,
    correlation_id: Uuid,
    required_features: &[&str],
) -> anyhow::Result<crate::commands::command_common::HelloSummary> {
    negotiate_hello_with_features(session, correlation_id, required_features).await
}

async fn send_bindings_cert_upload_request(
    command_name: &str,
    session: &Session,
    correlation_id: Uuid,
    public_key_hex: &str,
    authority: &str,
) -> anyhow::Result<()> {
    let request = deploy::request_envelope(
        MessageType::BindingsCertUpload,
        Uuid::new_v4(),
        correlation_id,
        &BindingsCertUploadRequest {
            public_key_hex: public_key_hex.to_string(),
            authority: authority.to_string(),
        },
    )?;
    let response: BindingsCertUploadResponse =
        deploy::response_payload(deploy::request_response(session, &request).await?)?;
    if !response.detail.is_empty() {
        ui::command_warn(command_name, &response.detail);
    }
    Ok(())
}

fn resolve_known_hosts_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("failed to resolve home directory for known_hosts"))?;
    Ok(home.join(IMAGO_DIR_NAME).join(KNOWN_HOSTS_FILE_NAME))
}

fn read_known_host_public_key(path: &Path, authority: &str) -> anyhow::Result<String> {
    let entries = load_known_hosts_entries(path)?;
    entries
        .get(authority)
        .cloned()
        .or_else(|| {
            if let Some(without_scheme) = authority.strip_prefix("rpc://") {
                entries
                    .get(without_scheme)
                    .cloned()
                    .or_else(|| entries.get(&without_scheme.to_ascii_lowercase()).cloned())
            } else {
                let normalized = format!("rpc://{authority}");
                entries
                    .get(&normalized)
                    .cloned()
                    .or_else(|| entries.get(&normalized.to_ascii_lowercase()).cloned())
            }
        })
        .ok_or_else(|| anyhow!("authority '{authority}' was not found in known_hosts"))
}

fn load_known_hosts_entries(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read known_hosts: {}", path.display()))?;
    let mut entries = BTreeMap::new();
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let (authority, key_hex) = trimmed.split_once('\t').ok_or_else(|| {
            anyhow!(
                "invalid known_hosts format at line {} in {}",
                index + 1,
                path.display()
            )
        })?;
        let normalized_key = normalize_ed25519_public_key_hex(key_hex).with_context(|| {
            format!(
                "invalid key at line {} in known_hosts {}",
                index + 1,
                path.display()
            )
        })?;
        if entries
            .insert(authority.to_string(), normalized_key)
            .is_some()
        {
            return Err(anyhow!(
                "duplicate authority '{}' in known_hosts {}",
                authority,
                path.display()
            ));
        }
    }

    Ok(entries)
}

fn normalize_known_hosts_authority(remote: &str) -> anyhow::Result<String> {
    let url = normalize_remote_to_url(remote)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("remote URL host is missing"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = url.port().unwrap_or(4443);
    Ok(format!(
        "rpc://{}:{}",
        format_host_for_url(&host).to_ascii_lowercase(),
        port
    ))
}

fn normalize_remote_to_url(remote: &str) -> anyhow::Result<Url> {
    if remote.contains("://") {
        return Url::parse(remote).context("remote URL parse failed");
    }

    if let Ok(address) = remote.parse::<SocketAddr>() {
        let host = format_host_for_url(&address.ip().to_string());
        return Url::parse(&format!("https://{}:{}/", host, address.port()))
            .context("remote socket address parse failed");
    }

    if let Ok(ip) = remote.parse::<IpAddr>() {
        let host = format_host_for_url(&ip.to_string());
        return Url::parse(&format!("https://{}:4443/", host))
            .context("remote ip address parse failed");
    }

    Url::parse(&format!("https://{remote}")).context("remote URL parse failed")
}

fn format_host_for_url(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn normalize_ed25519_public_key_hex(value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    let bytes = hex::decode(trimmed).with_context(|| format!("key is not valid hex: {trimmed}"))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "key must be a 32-byte ed25519 raw key (got {} bytes)",
            bytes.len()
        ));
    }
    Ok(hex::encode(bytes))
}

fn format_bindings_cert_deploy_result(from_error: Option<&str>, to_error: Option<&str>) -> String {
    let mut lines = vec!["bindings cert deploy completed with errors:".to_string()];
    lines.push(match from_error {
        Some(err) => format!("from: {err}"),
        None => "from: ok".to_string(),
    });
    lines.push(match to_error {
        Some(err) => format!("to: {err}"),
        None => "to: ok".to_string(),
    });
    lines.join("\n")
}

fn run_generate_inner(args: CertsGenerateArgs) -> anyhow::Result<GenerateOutput> {
    let out_dir = args.out_dir;
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create out dir: {}", out_dir.display()))?;

    let paths = OutputPaths {
        client_key: out_dir.join("client.key"),
        gitignore: out_dir.join(".gitignore"),
    };

    ensure_writable_targets(&paths, args.force)?;

    let client_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate client keypair")?;

    write_private_key(&paths.client_key, &client_key.serialize_pem())?;
    write_text(&paths.gitignore, GITIGNORE_CONTENT)?;

    Ok(GenerateOutput {
        paths,
        client_public_key_hex: hex::encode(client_key.public_key_raw()),
    })
}

fn ensure_writable_targets(paths: &OutputPaths, force: bool) -> anyhow::Result<()> {
    let all_paths = [&paths.client_key, &paths.gitignore];

    if force {
        return Ok(());
    }

    let mut existing = Vec::new();
    for path in all_paths {
        if path.exists() {
            existing.push(path.display().to_string());
        }
    }

    if existing.is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "output files already exist:\n{}\nrerun with --force to overwrite",
        existing.join("\n")
    ))
}

fn write_text(path: &Path, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(unix)]
fn write_private_key(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(not(unix))]
fn write_private_key(path: &Path, contents: &str) -> anyhow::Result<()> {
    write_text(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::BindingsCertUploadArgs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn generates_client_key_and_public_key_hex() {
        let dir = temp_dir("generates_client_key_and_public_key_hex");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            force: false,
        };

        let output = run_generate_inner(args).expect("key generation should succeed");

        assert!(output.paths.client_key.exists());
        assert!(output.paths.gitignore.exists());
        assert!(!dir.join("server.key").exists());
        assert!(!dir.join("server.pub.hex").exists());
        assert!(!dir.join("client.pub.hex").exists());

        let gitignore = std::fs::read_to_string(&output.paths.gitignore).expect("read .gitignore");
        assert_eq!(gitignore, GITIGNORE_CONTENT);

        assert_has_private_key(&output.paths.client_key);
        assert_public_key_matches_private(
            &output.paths.client_key,
            output.client_public_key_hex.as_str(),
        );

        cleanup(&dir);
    }

    #[test]
    fn fails_without_force_when_file_exists() {
        let dir = temp_dir("fails_without_force_when_file_exists");
        let existing = dir.join("client.key");
        std::fs::write(&existing, "dummy").expect("create existing file");

        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            force: false,
        };

        let err = run_generate_inner(args).expect_err("generation should fail");
        let message = err.to_string();
        assert!(message.contains("--force"));
        assert!(message.contains("client.key"));

        cleanup(&dir);
    }

    #[test]
    fn force_overwrites_existing_outputs() {
        let dir = temp_dir("force_overwrites_existing_outputs");
        let existing = dir.join("client.key");
        std::fs::write(&existing, "old").expect("create existing file");

        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            force: true,
        };

        let output = run_generate_inner(args).expect("generation with --force should succeed");
        let client_key = std::fs::read_to_string(output.paths.client_key).expect("read client key");
        assert!(client_key.contains("BEGIN PRIVATE KEY"));

        cleanup(&dir);
    }

    #[test]
    fn success_generate_result_sets_client_public_key_hex_meta() {
        let started_at = Instant::now();
        let public_key_hex =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();

        let result = success_generate_result(started_at, public_key_hex.clone());
        assert_eq!(
            result.meta.get("client_public_key_hex"),
            Some(&public_key_hex)
        );
    }

    #[tokio::test]
    async fn bindings_cert_upload_rejects_invalid_public_key_hex() {
        let dir = temp_dir("bindings_cert_upload_rejects_invalid_public_key_hex");
        let result = run_bindings_cert_upload_with_project_root(
            BindingsCertUploadArgs {
                public_key: "zz".to_string(),
                to: "rpc://node-a:4443".to_string(),
            },
            &dir,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .as_deref()
                .expect("stderr should be set")
                .contains("invalid PUBLIC_KEY_HEX")
        );

        cleanup(&dir);
    }

    #[test]
    fn normalizes_known_hosts_authority_from_rpc_url() {
        let authority =
            normalize_known_hosts_authority("rpc://Node-A.Example.com:9443").expect("valid url");
        assert_eq!(authority, "rpc://node-a.example.com:9443");
    }

    #[test]
    fn reads_known_host_public_key_from_file() {
        let dir = temp_dir("reads_known_host_public_key_from_file");
        let known_hosts_path = dir.join("known_hosts");
        let key = "a".repeat(64);
        let node_c_key = "b".repeat(64);
        std::fs::write(
            &known_hosts_path,
            format!("# comment\nnode-b:4443\t{key}\nnode-c:4443\t{node_c_key}\n"),
        )
        .expect("known_hosts should be written");

        let loaded = read_known_host_public_key(&known_hosts_path, "node-b:4443")
            .expect("key should be loaded");
        assert_eq!(loaded, key);

        cleanup(&dir);
    }

    #[test]
    fn load_known_hosts_entries_rejects_34_byte_key() {
        let dir = temp_dir("load_known_hosts_entries_rejects_34_byte_key");
        let known_hosts_path = dir.join("known_hosts");
        let key = "b".repeat(68);
        std::fs::write(&known_hosts_path, format!("node-c:4443\t{key}\n"))
            .expect("known_hosts should be written");

        let err =
            load_known_hosts_entries(&known_hosts_path).expect_err("34-byte key must be rejected");
        let message = format!("{err:#}");
        assert!(
            message.contains("key must be a 32-byte ed25519 raw key (got 34 bytes)"),
            "unexpected error: {message}"
        );

        cleanup(&dir);
    }

    #[test]
    fn reads_known_host_public_key_using_normalized_authority() {
        let dir = temp_dir("reads_known_host_public_key_using_normalized_authority");
        let known_hosts_path = dir.join("known_hosts");
        let key = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        std::fs::write(
            &known_hosts_path,
            format!("#\n# comment\nrpc://node-b:4443\t{key}\n"),
        )
        .expect("known_hosts should be written");

        let loaded = read_known_host_public_key(&known_hosts_path, "rpc://node-b:4443")
            .expect("key should be loaded");
        assert_eq!(loaded, key);

        cleanup(&dir);
    }

    #[test]
    fn formats_bindings_cert_deploy_result_with_from_to_labels() {
        let rendered =
            format_bindings_cert_deploy_result(Some("connect failed"), Some("upload failed"));
        assert!(rendered.contains("from: connect failed"));
        assert!(rendered.contains("to: upload failed"));
    }

    #[cfg(unix)]
    #[test]
    fn private_key_is_written_with_strict_permissions() {
        let dir = temp_dir("private_keys_are_written_with_strict_permissions");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            force: false,
        };

        let output = run_generate_inner(args).expect("key generation should succeed");
        assert_mode_0600(&output.paths.client_key);

        cleanup(&dir);
    }

    fn assert_public_key_matches_private(private_key_path: &Path, public_key_hex: &str) {
        let decoded = hex::decode(public_key_hex).expect("public key must be hex");
        assert_eq!(decoded.len(), 32, "ed25519 public key must be 32 bytes");
        let private_key_pem =
            std::fs::read_to_string(private_key_path).expect("private key should be readable");
        let key_pair = KeyPair::from_pem(&private_key_pem).expect("private key should parse");
        let expected = hex::encode(key_pair.public_key_raw());
        assert_eq!(public_key_hex, expected);
    }

    fn assert_has_private_key(path: &Path) {
        let file = std::fs::File::open(path).expect("open key");
        let mut reader = std::io::BufReader::new(file);
        let key = rustls_pemfile::private_key(&mut reader)
            .expect("parse key PEM")
            .expect("key should exist");
        let key_bytes = key.secret_der();
        assert!(
            !key_bytes.is_empty(),
            "key should not be empty: {}",
            path.display()
        );
    }

    fn temp_dir(test_name: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("imago-cli-certs-{test_name}-{ts}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    fn assert_mode_0600(path: &Path) {
        let mode = std::fs::metadata(path)
            .expect("metadata should be available")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "mode for {} should be 0600", path.display());
    }
}
