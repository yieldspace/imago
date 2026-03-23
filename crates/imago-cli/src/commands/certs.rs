use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, anyhow};
use imago_protocol::{
    BindingsCertInspectRequest, BindingsCertInspectResponse, BindingsCertUploadRequest,
    BindingsCertUploadResponse, MessageType,
};
use rcgen::{KeyPair, PKCS_ED25519};
use url::Url;
use uuid::Uuid;

use crate::{
    cli::{BindingsCertDeployArgs, BindingsCertUploadArgs, CertsGenerateArgs},
    commands::{
        build,
        command_common::{
            HelloSummary, format_local_context_line, format_peer_context_basic_line,
            negotiate_hello_with_features,
        },
        deploy,
        error_diagnostics::{format_command_error, summarize_command_failure},
        ui,
    },
    runtime,
};

use super::CommandResult;

const GITIGNORE_CONTENT: &str = "*\n!.gitignore\n";
const BINDINGS_CERT_INSPECT_FEATURE: &str = "bindings.cert.inspect";
const BINDINGS_CERT_UPLOAD_FEATURE: &str = "bindings.cert.upload";
const BINDINGS_CERT_INSPECT_REQUIRED_FEATURES: [&str; 1] = [BINDINGS_CERT_INSPECT_FEATURE];
const BINDINGS_CERT_UPLOAD_REQUIRED_FEATURES: [&str; 1] = [BINDINGS_CERT_UPLOAD_FEATURE];

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingsCertUploadSummary {
    authority: String,
    remote: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingsCertDeploySummary {
    from_authority: String,
    to_authority: String,
}

pub fn run_generate(args: CertsGenerateArgs) -> CommandResult {
    run_generate_with_project_root(args, Path::new("."))
}

pub(crate) fn run_generate_with_project_root(
    mut args: CertsGenerateArgs,
    project_root: &Path,
) -> CommandResult {
    if args.out_dir.is_relative() {
        args.out_dir = project_root.join(&args.out_dir);
    }
    let started_at = Instant::now();
    ui::command_start("trust.client-key.generate", "starting");
    ui::command_stage(
        "trust.client-key.generate",
        "generate",
        "creating key material",
    );
    match run_generate_inner(args) {
        Ok(output) => {
            let _ = runtime::write_stdout_line("generated key material:");
            let _ = runtime::write_stdout_line(&format!("  {}", output.paths.client_key.display()));
            let _ = runtime::write_stdout_line(&format!("  {}", output.paths.gitignore.display()));
            let _ = runtime::write_stdout_line(&format!(
                "  client_public_key_hex={}",
                output.client_public_key_hex.as_str()
            ));
            let _ = runtime::write_stdout_line(
                "private keys are sensitive. do not commit or share them.",
            );

            ui::command_finish("trust.client-key.generate", true, "");
            success_generate_result(started_at, output.client_public_key_hex)
        }
        Err(err) => {
            let summary_message = summarize_command_failure("trust.client-key.generate", &err);
            let diagnostic_message = format_command_error("trust.client-key.generate", &err);
            ui::command_finish("trust.client-key.generate", false, &summary_message);
            CommandResult::failure("trust.client-key.generate", started_at, diagnostic_message)
        }
    }
}

fn success_generate_result(started_at: Instant, client_public_key_hex: String) -> CommandResult {
    let mut result = CommandResult::success("trust.client-key.generate", started_at);
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
    ui::command_start("trust.cert.upload", "starting");
    match run_bindings_cert_upload_async(args, project_root).await {
        Ok(summary) => {
            ui::command_finish("trust.cert.upload", true, "");
            let mut result = CommandResult::success("trust.cert.upload", started_at);
            result
                .meta
                .insert("authority".to_string(), summary.authority);
            result.meta.insert("remote".to_string(), summary.remote);
            result
        }
        Err(err) => {
            let summary_message = summarize_command_failure("trust.cert.upload", &err);
            let diagnostic_message = format_command_error("trust.cert.upload", &err);
            ui::command_finish("trust.cert.upload", false, &summary_message);
            CommandResult::failure("trust.cert.upload", started_at, diagnostic_message)
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
    ui::command_start("trust.cert.replicate", "starting");
    match run_bindings_cert_deploy_async(args, project_root).await {
        Ok(summary) => {
            ui::command_finish("trust.cert.replicate", true, "");
            let mut result = CommandResult::success("trust.cert.replicate", started_at);
            result
                .meta
                .insert("from".to_string(), summary.from_authority);
            result.meta.insert("to".to_string(), summary.to_authority);
            result
        }
        Err(err) => {
            let summary_message = summarize_command_failure("trust.cert.replicate", &err);
            let diagnostic_message = format_command_error("trust.cert.replicate", &err);
            ui::command_finish("trust.cert.replicate", false, &summary_message);
            CommandResult::failure("trust.cert.replicate", started_at, diagnostic_message)
        }
    }
}

async fn run_bindings_cert_upload_async(
    args: BindingsCertUploadArgs,
    project_root: &Path,
) -> anyhow::Result<BindingsCertUploadSummary> {
    ui::command_stage("trust.cert.upload", "validate", "validating inputs");
    let public_key_hex = normalize_ed25519_public_key_hex(&args.public_key)
        .context("invalid PUBLIC_KEY_HEX for trust cert upload")?;
    let authority = normalize_rpc_authority(&args.authority)
        .context("invalid --authority for trust cert upload")?;
    ui::command_info(
        "trust.cert.upload",
        &format_local_context_line(
            project_root,
            "trust.cert.upload",
            build::default_target_name(),
            &args.to,
        ),
    );

    upload_public_key_to_remote("trust.cert.upload", &args.to, &public_key_hex, &authority).await?;
    Ok(BindingsCertUploadSummary {
        authority,
        remote: args.to,
    })
}

async fn run_bindings_cert_deploy_async(
    args: BindingsCertDeployArgs,
    project_root: &Path,
) -> anyhow::Result<BindingsCertDeploySummary> {
    ui::command_stage("trust.cert.replicate", "validate", "validating inputs");
    let from_authority = normalize_rpc_authority(&args.from_authority)
        .context("invalid --from-authority for trust cert replicate")?;
    let to_authority = normalize_rpc_authority(&args.to_authority)
        .context("invalid --to-authority for trust cert replicate")?;

    ui::command_info(
        "trust.cert.replicate",
        &format_local_context_line(
            project_root,
            "trust.cert.replicate.from",
            build::default_target_name(),
            &args.from,
        ),
    );
    let public_key_hex = inspect_public_key_from_remote("trust.cert.replicate", &args.from)
        .await
        .context("failed to inspect source daemon public key")?;

    ui::command_info(
        "trust.cert.replicate",
        &format_local_context_line(
            project_root,
            "trust.cert.replicate.to",
            build::default_target_name(),
            &args.to,
        ),
    );
    upload_public_key_to_remote(
        "trust.cert.replicate",
        &args.to,
        &public_key_hex,
        &from_authority,
    )
    .await
    .context("failed to upload source daemon public key to destination")?;

    Ok(BindingsCertDeploySummary {
        from_authority,
        to_authority,
    })
}

async fn inspect_public_key_from_remote(
    command_name: &str,
    remote: &str,
) -> anyhow::Result<String> {
    ui::command_stage(command_name, "connect", "connecting source admin endpoint");
    let connected = connect_remote(remote).await?;
    let _session_close_guard =
        deploy::ConnectedSessionCloseGuard::new(&connected, b"trust cert inspect complete");
    let correlation_id = Uuid::new_v4();

    ui::command_stage(command_name, "hello", "negotiating hello (source)");
    let _hello = negotiate_bindings_cert_hello(
        &connected,
        correlation_id,
        &BINDINGS_CERT_INSPECT_REQUIRED_FEATURES,
    )
    .await?;
    ui::command_info(
        command_name,
        &format_peer_context_basic_line(&connected.authority, &connected.resolved_addr),
    );

    ui::command_stage(command_name, "inspect", "requesting bindings.cert.inspect");
    let request = deploy::request_envelope(
        MessageType::BindingsCertInspect,
        Uuid::new_v4(),
        correlation_id,
        &BindingsCertInspectRequest {},
    )?;
    let response: BindingsCertInspectResponse =
        deploy::response_payload(deploy::request_response(&connected, &request).await?)?;
    normalize_ed25519_public_key_hex(&response.public_key_hex)
        .context("bindings.cert.inspect returned invalid public key")
}

async fn upload_public_key_to_remote(
    command_name: &str,
    remote: &str,
    public_key_hex: &str,
    authority: &str,
) -> anyhow::Result<()> {
    ui::command_stage(
        command_name,
        "connect",
        "connecting destination admin endpoint",
    );
    let connected = connect_remote(remote).await?;
    let _session_close_guard =
        deploy::ConnectedSessionCloseGuard::new(&connected, b"trust cert upload complete");
    let correlation_id = Uuid::new_v4();

    ui::command_stage(command_name, "hello", "negotiating hello (destination)");
    let _hello = negotiate_bindings_cert_hello(
        &connected,
        correlation_id,
        &BINDINGS_CERT_UPLOAD_REQUIRED_FEATURES,
    )
    .await?;
    ui::command_info(
        command_name,
        &format_peer_context_basic_line(&connected.authority, &connected.resolved_addr),
    );

    ui::command_stage(command_name, "upload", "uploading public key");
    send_bindings_cert_upload_request(
        command_name,
        &connected,
        correlation_id,
        public_key_hex,
        authority,
    )
    .await?;
    Ok(())
}

fn build_remote_target(remote: &str) -> anyhow::Result<build::DeployTargetConfig> {
    Ok(build::DeployTargetConfig {
        remote: remote.to_string(),
        ssh_remote: build::parse_target_remote(remote)
            .with_context(|| format!("invalid ssh target: {remote}"))?,
    })
}

async fn connect_remote(remote: &str) -> anyhow::Result<deploy::ConnectedTargetSession> {
    let target = build_remote_target(remote)?;
    runtime::connect_target(&target).await
}

async fn negotiate_bindings_cert_hello(
    session: &deploy::ConnectedTargetSession,
    correlation_id: Uuid,
    required_features: &[&str],
) -> anyhow::Result<HelloSummary> {
    negotiate_hello_with_features(session, correlation_id, required_features).await
}

async fn send_bindings_cert_upload_request(
    command_name: &str,
    session: &deploy::ConnectedTargetSession,
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

fn normalize_rpc_authority(raw: &str) -> anyhow::Result<String> {
    let parsed = Url::parse(raw).with_context(|| format!("authority URL parse failed: {raw}"))?;
    if parsed.scheme() != "rpc" {
        return Err(anyhow!("authority must use rpc:// scheme: {raw}"));
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(anyhow!(
            "authority must not include user credentials: {raw}"
        ));
    }
    if parsed.fragment().is_some() {
        return Err(anyhow!("authority must not include a fragment: {raw}"));
    }
    if parsed.query().is_some() {
        return Err(anyhow!(
            "authority must not include query parameters: {raw}"
        ));
    }
    if !parsed.path().is_empty() && parsed.path() != "/" {
        return Err(anyhow!("authority must not include a path: {raw}"));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("authority host is missing: {raw}"))?;
    let port = parsed
        .port()
        .ok_or_else(|| anyhow!("authority must include an explicit port: {raw}"))?;
    Ok(format!(
        "rpc://{}:{}",
        format_host_for_url(host).to_ascii_lowercase(),
        port
    ))
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
    use crate::runtime::{self, BufferedOutputSink, CliRuntime, OutputSink, SshTargetConnector};
    use rustls::pki_types::{PrivateKeyDer, pem::PemObject};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    use std::{path::Path, sync::Arc};

    fn plain_runtime(output_sink: Arc<dyn OutputSink>) -> Arc<CliRuntime> {
        Arc::new(CliRuntime::plain(
            Path::new("."),
            Arc::new(SshTargetConnector),
            output_sink,
        ))
    }

    fn capture_output(
        action: impl std::future::Future<Output = CommandResult>,
    ) -> (CommandResult, runtime::BufferedOutput) {
        let output_sink = Arc::new(BufferedOutputSink::default());
        let runtime = plain_runtime(output_sink.clone());
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build")
            .block_on(runtime::scope(runtime, action));
        (result, output_sink.snapshot())
    }

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

    #[test]
    fn run_generate_writes_generated_material_to_runtime_stdout() {
        let dir = temp_dir("run_generate_writes_generated_material_to_runtime_stdout");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            force: false,
        };

        let (result, output) = capture_output(async move { run_generate(args) });

        assert_eq!(result.exit_code, 0);
        assert!(output.stdout.contains("generated key material:\n"));
        assert!(output.stdout.contains("client.key"));
        assert!(output.stdout.contains("client_public_key_hex="));
        assert!(
            output
                .stdout
                .contains("private keys are sensitive. do not commit or share them.")
        );
        assert_eq!(output.stderr, "");

        cleanup(&dir);
    }

    #[test]
    fn run_generate_with_project_root_rebases_relative_out_dir() {
        let dir = temp_dir("run_generate_with_project_root_rebases_relative_out_dir");
        let args = CertsGenerateArgs {
            out_dir: PathBuf::from("certs"),
            force: false,
        };

        let result = run_generate_with_project_root(args, &dir);

        assert_eq!(result.exit_code, 0);
        assert!(dir.join("certs").join("client.key").exists());
        assert!(dir.join("certs").join(".gitignore").exists());

        cleanup(&dir);
    }

    #[tokio::test]
    async fn bindings_cert_upload_rejects_invalid_public_key_hex() {
        let dir = temp_dir("bindings_cert_upload_rejects_invalid_public_key_hex");
        let result = run_bindings_cert_upload_with_project_root(
            BindingsCertUploadArgs {
                public_key: "zz".to_string(),
                to: "ssh://localhost".to_string(),
                authority: "rpc://node-a:4443".to_string(),
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
    fn normalize_rpc_authority_lowercases_host() {
        let authority =
            normalize_rpc_authority("rpc://Node-A.Example.com:9443").expect("valid url");
        assert_eq!(authority, "rpc://node-a.example.com:9443");
    }

    #[test]
    fn normalize_rpc_authority_rejects_missing_port() {
        let err =
            normalize_rpc_authority("rpc://node-a.example.com").expect_err("port must be required");
        assert!(err.to_string().contains("explicit port"));
    }

    #[test]
    fn build_remote_target_rejects_non_ssh_remote() {
        let err =
            build_remote_target("rpc://node-a:4443").expect_err("rpc target must be rejected");
        assert!(err.to_string().contains("invalid ssh target"));
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
        let key = PrivateKeyDer::from_pem_reader(&mut reader).expect("parse key PEM");
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
