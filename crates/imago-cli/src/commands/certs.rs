use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use imago_protocol::{
    BindingsCertUploadRequest, BindingsCertUploadResponse, HelloNegotiateRequest,
    HelloNegotiateResponse, MessageType,
};
use rcgen::{KeyPair, PKCS_ED25519};
use url::Url;
use uuid::Uuid;
use web_transport_quinn::Session;

use crate::{
    cli::{BindingsCertDeployArgs, BindingsCertUploadArgs, CertsGenerateArgs},
    commands::{build, deploy},
};

use super::CommandResult;

const GITIGNORE_CONTENT: &str = "*\n!.gitignore\n";
const BINDINGS_CERT_UPLOAD_FEATURE: &str = "bindings.cert.upload";
const IMAGO_DIR_NAME: &str = ".imago";
const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts";

#[derive(Debug)]
struct OutputPaths {
    server_key: PathBuf,
    client_key: PathBuf,
    server_pub_hex: PathBuf,
    client_pub_hex: PathBuf,
    gitignore: PathBuf,
}

pub fn run_generate(args: CertsGenerateArgs) -> CommandResult {
    match run_generate_inner(args) {
        Ok(paths) => {
            println!("generated key material:");
            println!("  {}", paths.server_key.display());
            println!("  {}", paths.client_key.display());
            println!("  {}", paths.server_pub_hex.display());
            println!("  {}", paths.client_pub_hex.display());
            println!("  {}", paths.gitignore.display());
            println!("private keys are sensitive. do not commit or share them.");

            CommandResult {
                exit_code: 0,
                stderr: None,
            }
        }
        Err(err) => CommandResult {
            exit_code: 2,
            stderr: Some(err.to_string()),
        },
    }
}

pub async fn run_bindings_cert_upload(args: BindingsCertUploadArgs) -> CommandResult {
    run_bindings_cert_upload_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_bindings_cert_upload_with_project_root(
    args: BindingsCertUploadArgs,
    project_root: &Path,
) -> CommandResult {
    match run_bindings_cert_upload_async(args, project_root).await {
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

pub async fn run_bindings_cert_deploy(args: BindingsCertDeployArgs) -> CommandResult {
    run_bindings_cert_deploy_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_bindings_cert_deploy_with_project_root(
    args: BindingsCertDeployArgs,
    project_root: &Path,
) -> CommandResult {
    match run_bindings_cert_deploy_async(args, project_root).await {
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

async fn run_bindings_cert_upload_async(
    args: BindingsCertUploadArgs,
    project_root: &Path,
) -> anyhow::Result<()> {
    let public_key_hex = normalize_ed25519_public_key_hex(&args.public_key)
        .context("invalid PUBLIC_KEY_HEX for bindings cert upload")?;
    let authority = normalize_known_hosts_authority(&args.to)
        .with_context(|| format!("failed to normalize --to authority: {}", args.to))?;
    let client_key = load_bindings_client_key(project_root)?;

    upload_public_key_to_remote(&args.to, &public_key_hex, &authority, &client_key).await
}

async fn run_bindings_cert_deploy_async(
    args: BindingsCertDeployArgs,
    project_root: &Path,
) -> anyhow::Result<()> {
    let client_key = load_bindings_client_key(project_root)?;
    let mut from_failures = Vec::new();

    match connect_remote(&args.from, &client_key).await {
        Ok(session) => session.close(0, b"bindings cert deploy from probe complete"),
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
        upload_public_key_to_remote(&args.to, public_key_hex, &from_authority, &client_key)
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

async fn connect_remote(remote: &str, client_key: &Path) -> anyhow::Result<Session> {
    let target = build::DeployTargetConfig {
        remote: remote.to_string(),
        server_name: None,
        client_key: client_key.to_path_buf(),
    };
    deploy::connect_target(&target).await
}

async fn upload_public_key_to_remote(
    remote: &str,
    public_key_hex: &str,
    authority: &str,
    client_key: &Path,
) -> anyhow::Result<()> {
    let session = connect_remote(remote, client_key).await?;
    let correlation_id = Uuid::new_v4();

    negotiate_bindings_cert_upload_hello(&session, correlation_id).await?;
    send_bindings_cert_upload_request(&session, correlation_id, public_key_hex, authority).await?;
    session.close(0, b"bindings cert upload complete");

    Ok(())
}

async fn negotiate_bindings_cert_upload_hello(
    session: &Session,
    correlation_id: Uuid,
) -> anyhow::Result<()> {
    let hello_request = deploy::request_envelope(
        MessageType::HelloNegotiate,
        Uuid::new_v4(),
        correlation_id,
        &HelloNegotiateRequest {
            compatibility_date: deploy::COMPATIBILITY_DATE.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            required_features: vec![BINDINGS_CERT_UPLOAD_FEATURE.to_string()],
        },
    )?;
    let hello_response: HelloNegotiateResponse =
        deploy::response_payload(deploy::request_response(session, &hello_request).await?)?;
    if hello_response.accepted {
        return Ok(());
    }

    Err(anyhow!("hello.negotiate was rejected by server"))
}

async fn send_bindings_cert_upload_request(
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
        eprintln!("{}", response.detail);
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
        "{}:{}",
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

fn run_generate_inner(args: CertsGenerateArgs) -> anyhow::Result<OutputPaths> {
    let out_dir = args.out_dir;
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create out dir: {}", out_dir.display()))?;

    let paths = OutputPaths {
        server_key: out_dir.join("server.key"),
        client_key: out_dir.join("client.key"),
        server_pub_hex: out_dir.join("server.pub.hex"),
        client_pub_hex: out_dir.join("client.pub.hex"),
        gitignore: out_dir.join(".gitignore"),
    };

    ensure_writable_targets(&paths, args.force)?;

    let server_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate server keypair")?;
    let client_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate client keypair")?;

    write_private_key(&paths.server_key, &server_key.serialize_pem())?;
    write_private_key(&paths.client_key, &client_key.serialize_pem())?;
    write_text(
        &paths.server_pub_hex,
        &format!("{}\n", hex::encode(server_key.public_key_raw())),
    )?;
    write_text(
        &paths.client_pub_hex,
        &format!("{}\n", hex::encode(client_key.public_key_raw())),
    )?;
    write_text(&paths.gitignore, GITIGNORE_CONTENT)?;

    Ok(paths)
}

fn ensure_writable_targets(paths: &OutputPaths, force: bool) -> anyhow::Result<()> {
    let all_paths = [
        &paths.server_key,
        &paths.client_key,
        &paths.server_pub_hex,
        &paths.client_pub_hex,
        &paths.gitignore,
    ];

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
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn generates_all_files_and_valid_payloads() {
        let dir = temp_dir("generates_all_files_and_valid_payloads");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: false,
        };

        let paths = run_generate_inner(args).expect("key generation should succeed");

        assert!(paths.server_key.exists());
        assert!(paths.client_key.exists());
        assert!(paths.server_pub_hex.exists());
        assert!(paths.client_pub_hex.exists());
        assert!(paths.gitignore.exists());

        let gitignore = std::fs::read_to_string(&paths.gitignore).expect("read .gitignore");
        assert_eq!(gitignore, GITIGNORE_CONTENT);

        assert_has_private_key(&paths.server_key);
        assert_has_private_key(&paths.client_key);
        assert_public_key_hex(&paths.server_pub_hex);
        assert_public_key_hex(&paths.client_pub_hex);

        assert_public_key_matches_private(&paths.server_key, &paths.server_pub_hex);
        assert_public_key_matches_private(&paths.client_key, &paths.client_pub_hex);

        cleanup(&dir);
    }

    #[test]
    fn fails_without_force_when_file_exists() {
        let dir = temp_dir("fails_without_force_when_file_exists");
        let existing = dir.join("server.key");
        std::fs::write(&existing, "dummy").expect("create existing file");

        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: false,
        };

        let err = run_generate_inner(args).expect_err("generation should fail");
        let message = err.to_string();
        assert!(message.contains("--force"));
        assert!(message.contains("server.key"));

        cleanup(&dir);
    }

    #[test]
    fn force_overwrites_existing_outputs() {
        let dir = temp_dir("force_overwrites_existing_outputs");
        let existing = dir.join("server.key");
        std::fs::write(&existing, "old").expect("create existing file");

        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: true,
        };

        let paths = run_generate_inner(args).expect("generation with --force should succeed");
        let server_key = std::fs::read_to_string(paths.server_key).expect("read server key");
        assert!(server_key.contains("BEGIN PRIVATE KEY"));

        cleanup(&dir);
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
        assert_eq!(authority, "node-a.example.com:9443");
    }

    #[test]
    fn reads_known_host_public_key_from_file() {
        let dir = temp_dir("reads_known_host_public_key_from_file");
        let known_hosts_path = dir.join("known_hosts");
        let key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        std::fs::write(
            &known_hosts_path,
            format!(
                "# comment\nnode-b:4443\t{key}\nnode-c:4443\tbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n"
            ),
        )
        .expect("known_hosts should be written");

        let loaded = read_known_host_public_key(&known_hosts_path, "node-b:4443")
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
    fn private_keys_are_written_with_strict_permissions() {
        let dir = temp_dir("private_keys_are_written_with_strict_permissions");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: false,
        };

        let paths = run_generate_inner(args).expect("key generation should succeed");
        assert_mode_0600(&paths.server_key);
        assert_mode_0600(&paths.client_key);

        cleanup(&dir);
    }

    fn assert_public_key_hex(path: &Path) {
        let value = std::fs::read_to_string(path).expect("public key file should be readable");
        let trimmed = value.trim();
        let decoded = hex::decode(trimmed).expect("public key must be hex");
        assert_eq!(decoded.len(), 32, "ed25519 public key must be 32 bytes");
    }

    fn assert_public_key_matches_private(private_key_path: &Path, public_key_hex_path: &Path) {
        let private_key_pem =
            std::fs::read_to_string(private_key_path).expect("private key should be readable");
        let key_pair = KeyPair::from_pem(&private_key_pem).expect("private key should parse");
        let expected = hex::encode(key_pair.public_key_raw());

        let actual = std::fs::read_to_string(public_key_hex_path)
            .expect("public key should be readable")
            .trim()
            .to_string();
        assert_eq!(actual, expected);
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
