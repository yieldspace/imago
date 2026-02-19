use std::{
    collections::BTreeMap,
    fs,
    io::{BufReader, Read, Write},
    net::{IpAddr, SocketAddr},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, anyhow};
use base64::Engine;
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushChunkHeader, ArtifactPushRequest,
    ArtifactStatus, ByteRange, CommandEvent, CommandEventType, CommandPayload, CommandStartRequest,
    CommandStartResponse, CommandType, DeployCommandPayload, DeployPrepareRequest,
    DeployPrepareResponse, ErrorCode, HelloNegotiateRequest, HelloNegotiateResponse, MessageType,
    ProtocolEnvelope, StructuredError, from_cbor, to_cbor,
};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::{
        AlwaysResolvesClientRawPublicKeys,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
    sign::CertifiedKey,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    net::lookup_host,
    task::JoinSet,
};
use url::Url;
use uuid::Uuid;
use web_transport_quinn::{Session, proto::ConnectRequest};

use crate::{
    cli::DeployArgs,
    commands::{
        CommandResult, build,
        shared::dependency::{DependencyResolver, StandardDependencyResolver},
    },
};

mod artifact;
mod network;

const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const COMPATIBILITY_DATE: &str = "2026-02-10";
const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
const DEFAULT_MAX_INFLIGHT_CHUNKS: usize = 16;
const TRANSPORT_CONNECT_STAGE: &str = "transport.connect";
const UPLOAD_MAX_ATTEMPTS: usize = 4;
const UPLOAD_RETRY_BASE_BACKOFF_MS: u64 = 250;
const UPLOAD_RETRY_MAX_BACKOFF_MS: u64 = 1000;
const DEFAULT_DEPLOY_STREAM_TIMEOUT_SECS: u64 = 30;
const DEPLOY_STREAM_RETRY_BACKOFF_MS: [u64; 2] = [100, 250];
const DEPLOY_STREAM_MAX_ATTEMPTS: usize = DEPLOY_STREAM_RETRY_BACKOFF_MS.len() + 1;
const DATAGRAM_BUFFER_BYTES: usize = 1024 * 1024;
const IMAGO_DIR_NAME: &str = ".imago";
const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts";
#[cfg(unix)]
const IMAGO_DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const KNOWN_HOSTS_MODE: u32 = 0o600;
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

pub(crate) type Envelope = ProtocolEnvelope<Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UploadLimits {
    chunk_size: usize,
    max_inflight_chunks: usize,
    deploy_stream_timeout: Duration,
}

#[derive(Clone, Copy)]
struct UploadRequestContext<'a> {
    session: &'a web_transport_quinn::Session,
    correlation_id: Uuid,
    deploy_id: &'a str,
    upload_token: &'a str,
    stream_timeout: Duration,
}

struct UploadPhaseInputs<'a> {
    target: &'a build::DeployTargetConfig,
    target_for_protocol: &'a BTreeMap<String, String>,
    policy: &'a BTreeMap<String, String>,
    manifest: &'a Manifest,
    artifact_path: &'a Path,
    artifact_digest: &'a str,
    artifact_size: u64,
    manifest_digest: &'a str,
    idempotency_key: &'a str,
    correlation_id: Uuid,
}

struct UploadPhaseResult {
    session: Session,
    deploy_id: String,
    deploy_stream_timeout: Duration,
}

#[derive(Debug, Clone)]
struct ServerResponseError {
    error: StructuredError,
}

impl std::fmt::Display for ServerResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "server error: {} ({:?}) at {}",
            self.error.message, self.error.code, self.error.stage
        )
    }
}

impl std::error::Error for ServerResponseError {}

#[derive(Debug)]
struct CommitNotVerifiedError;

impl std::fmt::Display for CommitNotVerifiedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "artifact.commit returned verified=false")
    }
}

impl std::error::Error for CommitNotVerifiedError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerIdentityStatus {
    Matched {
        presented_key_hex: String,
    },
    Unknown {
        presented_key_hex: String,
    },
    Mismatch {
        expected_key_hex: String,
        presented_key_hex: String,
    },
}

#[derive(Debug)]
struct TofuServerCertVerifier {
    provider: Arc<CryptoProvider>,
    expected_key_hex: Option<String>,
    observed_status: Mutex<Option<ServerIdentityStatus>>,
}

impl TofuServerCertVerifier {
    fn new(provider: Arc<CryptoProvider>, expected_key_hex: Option<String>) -> Self {
        Self {
            provider,
            expected_key_hex: expected_key_hex.map(|value| value.to_ascii_lowercase()),
            observed_status: Mutex::new(None),
        }
    }

    fn take_observed_status(&self) -> Option<ServerIdentityStatus> {
        self.observed_status
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
    }
}

impl ServerCertVerifier for TofuServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let raw_key = extract_ed25519_raw_public_key_from_spki(end_entity.as_ref())
            .map_err(|err| rustls::Error::General(err.to_string()))?;
        let presented_key_hex = hex::encode(raw_key);

        let status = match &self.expected_key_hex {
            Some(expected_key_hex) if expected_key_hex.eq_ignore_ascii_case(&presented_key_hex) => {
                ServerIdentityStatus::Matched { presented_key_hex }
            }
            Some(expected_key_hex) => ServerIdentityStatus::Mismatch {
                expected_key_hex: expected_key_hex.clone(),
                presented_key_hex,
            },
            None => ServerIdentityStatus::Unknown { presented_key_hex },
        };
        if let Ok(mut guard) = self.observed_status.lock() {
            *guard = Some(status);
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General(
            "TLS1.2 server signatures are not supported for raw public keys".to_string(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature_with_raw_key(
            message,
            &rustls::pki_types::SubjectPublicKeyInfoDer::from(cert.as_ref()),
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    main: String,
    #[serde(rename = "type")]
    app_type: String,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
    #[serde(default)]
    dependencies: Vec<build::ManifestDependency>,
}

#[derive(Debug, Deserialize)]
struct ManifestAsset {
    path: String,
}

#[derive(Debug)]
struct TempArtifactBundle {
    path: PathBuf,
}

impl TempArtifactBundle {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempArtifactBundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn run(args: DeployArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(args: DeployArgs, project_root: &Path) -> CommandResult {
    run_with_project_root_and_target_override(args, project_root, None).await
}

pub(crate) async fn run_with_project_root_and_target_override(
    args: DeployArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
) -> CommandResult {
    match run_async_with_target_override(args, project_root, target_override).await {
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

async fn run_async_with_target_override(
    args: DeployArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
) -> anyhow::Result<()> {
    let dependency_resolver = StandardDependencyResolver;
    let target_connector = network::QuinnTargetConnector;
    let artifact_bundler = artifact::TarArtifactBundler;

    let target_name = args
        .target
        .clone()
        .unwrap_or_else(|| build::default_target_name().to_string());
    let build_output =
        build::build_project_with_target_override(&target_name, project_root, target_override)
            .context("failed to run build before deploy")?;

    let manifest_path = build_output.manifest_path;
    let manifest_bytes = build_output.manifest_bytes;
    let restart_policy = build_output.restart_policy;
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("failed to parse manifest json")?;
    let dependency_component_sources = dependency_resolver
        .resolve_dependency_component_sources(project_root, &manifest.dependencies)?;

    let target = build_output
        .target
        .require_deploy_credentials()
        .context("target settings are invalid for deploy")?;

    let artifact = artifact::ArtifactBundler::bundle(
        &artifact_bundler,
        artifact::ArtifactBundleRequest {
            manifest: &manifest,
            manifest_path: &manifest_path,
            project_root,
            dependency_component_sources: &dependency_component_sources,
        },
    )?;
    let (artifact_digest, artifact_size) = compute_file_sha256_and_size(artifact.path())?;
    let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
    let correlation_id = Uuid::new_v4();
    let target_for_protocol = normalize_target_for_protocol(&target);
    let policy = BTreeMap::new();
    let idempotency_key = build_idempotency_key(
        &manifest.name,
        &manifest.app_type,
        &target_for_protocol,
        &policy,
        &artifact_digest,
        artifact_size,
        &manifest_digest,
    );

    let upload_result = run_upload_phase_with_resume(
        &target_connector,
        UploadPhaseInputs {
            target: &target,
            target_for_protocol: &target_for_protocol,
            policy: &policy,
            manifest: &manifest,
            artifact_path: artifact.path(),
            artifact_digest: &artifact_digest,
            artifact_size,
            manifest_digest: &manifest_digest,
            idempotency_key: &idempotency_key,
            correlation_id,
        },
    )
    .await?;

    let command_request_id = Uuid::new_v4();
    let command = build_command_start_envelope(
        correlation_id,
        command_request_id,
        CommandType::Deploy,
        CommandPayload::Deploy(DeployCommandPayload {
            deploy_id: upload_result.deploy_id.clone(),
            expected_current_release: "any".to_string(),
            restart_policy,
            auto_rollback: true,
        }),
    )?;

    let responses = request_events_with_timeout(
        &upload_result.session,
        &command,
        upload_result.deploy_stream_timeout,
    )
    .await?;
    if responses.is_empty() {
        return Err(anyhow!("command.start returned empty response stream"));
    }

    let start_response: CommandStartResponse = response_payload(responses[0].clone())?;
    if !start_response.accepted {
        return Err(anyhow!("command.start was not accepted"));
    }

    let mut terminal: Option<CommandEvent> = None;
    for envelope in responses.iter().skip(1) {
        if envelope.message_type != MessageType::CommandEvent {
            continue;
        }
        let event: CommandEvent = response_payload(envelope.clone())?;
        if let Some(stage) = &event.stage {
            eprintln!("event={:?} stage={}", event.event_type, stage);
        }
        if matches!(
            event.event_type,
            CommandEventType::Succeeded | CommandEventType::Failed | CommandEventType::Canceled
        ) {
            terminal = Some(event);
            break;
        }
    }

    let terminal =
        terminal.ok_or_else(|| anyhow!("command.event terminal event was not received"))?;
    match terminal.event_type {
        CommandEventType::Succeeded => Ok(()),
        CommandEventType::Failed => {
            if let Some(err) = terminal.error {
                Err(anyhow!(
                    "deploy failed: {} ({:?}) at {}",
                    err.message,
                    err.code,
                    err.stage
                ))
            } else {
                Err(anyhow!("deploy failed without structured error"))
            }
        }
        CommandEventType::Canceled => Err(anyhow!("deploy was canceled")),
        _ => Err(anyhow!("unexpected terminal event")),
    }
}

async fn run_upload_phase_with_resume<C: network::TargetConnector>(
    target_connector: &C,
    inputs: UploadPhaseInputs<'_>,
) -> anyhow::Result<UploadPhaseResult> {
    for attempt in 1..=UPLOAD_MAX_ATTEMPTS {
        match run_upload_phase_once(target_connector, &inputs).await {
            Ok(result) => return Ok(result),
            Err(err) => {
                if attempt >= UPLOAD_MAX_ATTEMPTS || !should_retry_upload_error(&err) {
                    return Err(err.context(format!(
                        "upload phase failed on attempt {attempt}/{UPLOAD_MAX_ATTEMPTS}"
                    )));
                }

                let backoff = retry_backoff_duration(attempt);
                eprintln!(
                    "{}",
                    format_retry_log_message(
                        attempt,
                        UPLOAD_MAX_ATTEMPTS,
                        backoff,
                        &summarize_retry_error(&err),
                    )
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }

    Err(anyhow!(
        "upload retry loop exhausted unexpectedly without a terminal result"
    ))
}

async fn run_upload_phase_once<C: network::TargetConnector>(
    target_connector: &C,
    inputs: &UploadPhaseInputs<'_>,
) -> anyhow::Result<UploadPhaseResult> {
    let session = target_connector.connect(inputs.target).await?;

    let hello = request_envelope(
        MessageType::HelloNegotiate,
        Uuid::new_v4(),
        inputs.correlation_id,
        &HelloNegotiateRequest {
            compatibility_date: COMPATIBILITY_DATE.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            required_features: vec![
                "deploy.prepare".to_string(),
                "artifact.push".to_string(),
                "artifact.commit".to_string(),
                "command.start".to_string(),
                "command.event".to_string(),
            ],
        },
    )?;
    let hello_response: HelloNegotiateResponse =
        response_payload(request_response(&session, &hello).await?)?;
    if !hello_response.accepted {
        return Err(anyhow!("hello.negotiate was rejected by server"));
    }
    let upload_limits = parse_upload_limits(&hello_response)?;

    let prepare = request_envelope(
        MessageType::DeployPrepare,
        Uuid::new_v4(),
        inputs.correlation_id,
        &DeployPrepareRequest {
            name: inputs.manifest.name.clone(),
            app_type: inputs.manifest.app_type.clone(),
            target: inputs.target_for_protocol.clone(),
            artifact_digest: inputs.artifact_digest.to_string(),
            artifact_size: inputs.artifact_size,
            manifest_digest: inputs.manifest_digest.to_string(),
            idempotency_key: inputs.idempotency_key.to_string(),
            policy: inputs.policy.clone(),
        },
    )?;
    let prepare_response: DeployPrepareResponse = response_payload(
        request_response_with_timeout(&session, &prepare, upload_limits.deploy_stream_timeout)
            .await?,
    )?;

    let upload_ranges = upload_ranges_for_prepare(
        prepare_response.artifact_status,
        &prepare_response.missing_ranges,
        inputs.artifact_size,
    )?;
    if !upload_ranges.is_empty() {
        let upload_context = UploadRequestContext {
            session: &session,
            correlation_id: inputs.correlation_id,
            deploy_id: &prepare_response.deploy_id,
            upload_token: &prepare_response.upload_token,
            stream_timeout: upload_limits.deploy_stream_timeout,
        };
        push_artifact_ranges(
            upload_context,
            inputs.artifact_path,
            inputs.artifact_size,
            &upload_ranges,
            upload_limits,
        )
        .await?;
    }

    let commit = request_envelope(
        MessageType::ArtifactCommit,
        Uuid::new_v4(),
        inputs.correlation_id,
        &ArtifactCommitRequest {
            deploy_id: prepare_response.deploy_id.clone(),
            artifact_digest: inputs.artifact_digest.to_string(),
            artifact_size: inputs.artifact_size,
            manifest_digest: inputs.manifest_digest.to_string(),
        },
    )?;
    let commit_response: ArtifactCommitResponse = response_payload(
        request_response_with_timeout(&session, &commit, upload_limits.deploy_stream_timeout)
            .await?,
    )?;
    if !commit_response.verified {
        return Err(CommitNotVerifiedError.into());
    }

    Ok(UploadPhaseResult {
        session,
        deploy_id: prepare_response.deploy_id,
        deploy_stream_timeout: upload_limits.deploy_stream_timeout,
    })
}

fn should_retry_upload_error(err: &anyhow::Error) -> bool {
    if contains_commit_not_verified_error(err) {
        return false;
    }

    match find_server_response_error(err) {
        Some(server_error) => {
            if is_non_retryable_error_code(server_error.error.code) {
                return false;
            }
            if server_error.error.code == ErrorCode::Busy {
                return true;
            }
            server_error.error.retryable
        }
        None => !contains_unauthorized_marker(err),
    }
}

fn retry_backoff_duration(retry_index: usize) -> Duration {
    let shift = retry_index.saturating_sub(1).min(8) as u32;
    let factor = 1u64 << shift;
    let millis = UPLOAD_RETRY_BASE_BACKOFF_MS
        .saturating_mul(factor)
        .min(UPLOAD_RETRY_MAX_BACKOFF_MS);
    Duration::from_millis(millis)
}

fn summarize_retry_error(err: &anyhow::Error) -> String {
    if let Some(server_error) = find_server_response_error(err) {
        return format!(
            "{:?} at {}",
            server_error.error.code, server_error.error.stage
        );
    }
    truncate_log_message(&err.to_string(), 160)
}

fn format_retry_log_message(
    attempt: usize,
    total_attempts: usize,
    backoff: Duration,
    reason: &str,
) -> String {
    format!(
        "upload attempt {attempt}/{total_attempts} failed, retrying in {}ms (reason={reason})",
        backoff.as_millis()
    )
}

fn truncate_log_message(message: &str, max_chars: usize) -> String {
    let len = message.chars().count();
    if len <= max_chars {
        return message.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let head = message
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    format!("{head}...")
}

fn find_server_response_error(err: &anyhow::Error) -> Option<&ServerResponseError> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<ServerResponseError>())
}

fn contains_commit_not_verified_error(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<CommitNotVerifiedError>().is_some())
}

fn is_non_retryable_error_code(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::Unauthorized
            | ErrorCode::BadRequest
            | ErrorCode::BadManifest
            | ErrorCode::IdempotencyConflict
            | ErrorCode::RangeInvalid
            | ErrorCode::ChunkHashMismatch
            | ErrorCode::StorageQuota
            | ErrorCode::PreconditionFailed
    )
}

fn contains_unauthorized_marker(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().contains("E_UNAUTHORIZED"))
}

pub(crate) async fn connect_target(target: &build::DeployTargetConfig) -> anyhow::Result<Session> {
    let client_key = load_private_key(&target.client_key)?;
    let endpoint_info = parse_remote_endpoint(&target.remote).await?;
    let configured_host = target.server_name.as_deref().unwrap_or(&endpoint_info.host);
    let authority = format_authority(configured_host, endpoint_info.port);
    let known_hosts_path = known_hosts_path()?;
    let known_hosts_entries = load_known_hosts_entries(&known_hosts_path)?;
    let expected_key_hex = known_hosts_entries.get(&authority).cloned();

    let provider = web_transport_quinn::crypto::default_provider();
    let verifier = Arc::new(TofuServerCertVerifier::new(
        provider.clone(),
        expected_key_hex,
    ));
    let client_resolver = build_client_raw_public_key_resolver(provider.clone(), &client_key)?;

    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_client_cert_resolver(client_resolver);
    tls.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];

    let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let mut quic_config = quinn::ClientConfig::new(Arc::new(quic_tls));
    let mut transport = quinn::TransportConfig::default();
    transport.datagram_send_buffer_size(DATAGRAM_BUFFER_BYTES);
    transport.datagram_receive_buffer_size(Some(DATAGRAM_BUFFER_BYTES));
    quic_config.transport_config(Arc::new(transport));
    let endpoint = create_client_endpoint()?;

    let sni = configured_host.to_string();
    let connecting = endpoint
        .connect_with(quic_config, endpoint_info.remote_addr, &sni)
        .map_err(|e| anyhow!("failed to start quic connection: {e}"))?;
    let connection = connecting.await.map_err(map_connect_connection_error)?;

    let request_host = format_host_for_url(configured_host);
    let request_url = Url::parse(&format!("https://{}:{}/", request_host, endpoint_info.port))
        .context("failed to build webtransport request URL")?;
    let request = ConnectRequest::new(request_url);
    let session = Session::connect(connection, request)
        .await
        .map_err(map_webtransport_client_error)?;

    match verifier.take_observed_status() {
        Some(ServerIdentityStatus::Matched { .. }) => Ok(session),
        Some(ServerIdentityStatus::Unknown { presented_key_hex }) => {
            if let Err(err) =
                save_known_host_entry(&known_hosts_path, &authority, &presented_key_hex)
            {
                session.close(0, b"failed to persist known_hosts entry");
                return Err(err);
            }
            Ok(session)
        }
        Some(ServerIdentityStatus::Mismatch {
            expected_key_hex,
            presented_key_hex,
        }) => {
            session.close(0, b"server raw public key mismatch");
            Err(server_identity_mismatch_error(
                &authority,
                &expected_key_hex,
                &presented_key_hex,
                &known_hosts_path,
            ))
        }
        None => {
            session.close(0, b"missing server identity verification");
            Err(missing_server_identity_error(&authority))
        }
    }
}

fn server_identity_mismatch_error(
    authority: &str,
    expected_key_hex: &str,
    presented_key_hex: &str,
    known_hosts_path: &Path,
) -> anyhow::Error {
    unauthorized_connect_error(format!(
        "server key mismatch for authority '{authority}': expected {expected_key_hex}, got {presented_key_hex}. if the server key changed intentionally, edit {} manually and retry",
        known_hosts_path.display()
    ))
}

fn missing_server_identity_error(authority: &str) -> anyhow::Error {
    unauthorized_connect_error(format!(
        "failed to verify server raw public key for authority: {authority}"
    ))
}

fn is_certificate_alert_transport_code(code: quinn::TransportErrorCode) -> bool {
    let raw_code = u64::from(code);
    if !(0x100..0x200).contains(&raw_code) {
        return false;
    }
    let alert_code = (raw_code & 0xff) as u8;
    is_certificate_alert_code(alert_code)
}

fn is_certificate_alert_code(alert_code: u8) -> bool {
    matches!(
        alert_code,
        code if code == u8::from(rustls::AlertDescription::NoCertificate)
            || code == u8::from(rustls::AlertDescription::HandshakeFailure)
            || code == u8::from(rustls::AlertDescription::BadCertificate)
            || code == u8::from(rustls::AlertDescription::UnsupportedCertificate)
            || code == u8::from(rustls::AlertDescription::CertificateRevoked)
            || code == u8::from(rustls::AlertDescription::CertificateExpired)
            || code == u8::from(rustls::AlertDescription::CertificateUnknown)
            || code == u8::from(rustls::AlertDescription::UnknownCA)
            || code == u8::from(rustls::AlertDescription::AccessDenied)
            || code == u8::from(rustls::AlertDescription::BadCertificateStatusResponse)
            || code == u8::from(rustls::AlertDescription::BadCertificateHashValue)
            || code == u8::from(rustls::AlertDescription::CertificateRequired)
    )
}

fn is_certificate_auth_connection_error(err: &quinn::ConnectionError) -> bool {
    match err {
        quinn::ConnectionError::TransportError(transport_error) => {
            is_certificate_alert_transport_code(transport_error.code)
        }
        quinn::ConnectionError::ConnectionClosed(close) => {
            is_certificate_alert_transport_code(close.error_code)
        }
        _ => false,
    }
}

fn unauthorized_connect_error(source: impl std::fmt::Display) -> anyhow::Error {
    anyhow!(
        "server error: public key authentication failed (E_UNAUTHORIZED) at {TRANSPORT_CONNECT_STAGE}: {source}"
    )
}

fn map_connect_rejection_status(
    status: web_transport_quinn::http::StatusCode,
    source: impl std::fmt::Display,
) -> Option<anyhow::Error> {
    if status == web_transport_quinn::http::StatusCode::UNAUTHORIZED
        || status == web_transport_quinn::http::StatusCode::FORBIDDEN
    {
        return Some(unauthorized_connect_error(source));
    }
    None
}

fn parse_connect_error_status(message: &str) -> Option<web_transport_quinn::http::StatusCode> {
    let (_, status_with_reason) = message.rsplit_once("http error status: ")?;
    let status_token = status_with_reason.split_ascii_whitespace().next()?;
    let code = status_token.parse::<u16>().ok()?;
    web_transport_quinn::http::StatusCode::from_u16(code).ok()
}

fn map_connect_connection_error(err: quinn::ConnectionError) -> anyhow::Error {
    if is_certificate_auth_connection_error(&err) {
        return unauthorized_connect_error(err);
    }
    anyhow!("failed to establish quic connection: {err}")
}

fn map_webtransport_client_error(err: web_transport_quinn::ClientError) -> anyhow::Error {
    match err {
        web_transport_quinn::ClientError::Connection(connection_err) => {
            map_connect_connection_error(connection_err)
        }
        web_transport_quinn::ClientError::HttpError(connect_err) => {
            let rendered = connect_err.to_string();
            if let Some(status) = parse_connect_error_status(&rendered)
                && let Some(mapped) = map_connect_rejection_status(status, &rendered)
            {
                return mapped;
            }
            anyhow!("failed to establish webtransport session: {connect_err}")
        }
        other => anyhow!("failed to establish webtransport session: {other}"),
    }
}

fn parse_upload_limits(response: &HelloNegotiateResponse) -> anyhow::Result<UploadLimits> {
    let chunk_size = parse_positive_limit(
        &response.limits,
        "chunk_size",
        DEFAULT_CHUNK_SIZE,
        "hello.negotiate response chunk_size",
    )?;
    let max_inflight_chunks = parse_positive_limit(
        &response.limits,
        "max_inflight_chunks",
        DEFAULT_MAX_INFLIGHT_CHUNKS,
        "hello.negotiate response max_inflight_chunks",
    )?;
    let deploy_stream_timeout_secs = parse_positive_limit_u64(
        &response.limits,
        "deploy_stream_timeout_secs",
        DEFAULT_DEPLOY_STREAM_TIMEOUT_SECS,
        "hello.negotiate response deploy_stream_timeout_secs",
    )?;

    Ok(UploadLimits {
        chunk_size,
        max_inflight_chunks,
        deploy_stream_timeout: Duration::from_secs(deploy_stream_timeout_secs),
    })
}

fn parse_positive_limit(
    limits: &BTreeMap<String, String>,
    key: &str,
    default: usize,
    label: &str,
) -> anyhow::Result<usize> {
    match limits.get(key) {
        Some(raw) => {
            let parsed = raw
                .parse::<usize>()
                .with_context(|| format!("failed to parse {label} as integer: {raw}"))?;
            if parsed == 0 {
                return Err(anyhow!("{label} must be greater than 0"));
            }
            Ok(parsed)
        }
        None => Ok(default),
    }
}

fn parse_positive_limit_u64(
    limits: &BTreeMap<String, String>,
    key: &str,
    default: u64,
    label: &str,
) -> anyhow::Result<u64> {
    match limits.get(key) {
        Some(raw) => {
            let parsed = raw
                .parse::<u64>()
                .with_context(|| format!("failed to parse {label} as integer: {raw}"))?;
            if parsed == 0 {
                return Err(anyhow!("{label} must be greater than 0"));
            }
            Ok(parsed)
        }
        None => Ok(default),
    }
}

fn client_bind_candidates() -> [SocketAddr; 2] {
    [
        "[::]:0".parse().expect("valid ipv6 wildcard address"),
        "0.0.0.0:0".parse().expect("valid ipv4 wildcard address"),
    ]
}

fn create_client_endpoint() -> anyhow::Result<quinn::Endpoint> {
    let mut last_error: Option<anyhow::Error> = None;
    for bind_addr in client_bind_candidates() {
        match quinn::Endpoint::client(bind_addr) {
            Ok(endpoint) => return Ok(endpoint),
            Err(err) => {
                last_error = Some(anyhow!(
                    "failed to bind client endpoint on {bind_addr}: {err}"
                ));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("failed to bind client endpoint")))
}

struct RemoteEndpoint {
    remote_addr: SocketAddr,
    host: String,
    port: u16,
}

async fn parse_remote_endpoint(remote: &str) -> anyhow::Result<RemoteEndpoint> {
    let url = normalize_remote_to_url(remote)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("remote URL host is missing"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = url.port().unwrap_or(4443);
    let mut addresses = lookup_host((host.clone(), port))
        .await
        .with_context(|| format!("failed to resolve remote host: {host}:{port}"))?;
    let addr = addresses
        .next()
        .ok_or_else(|| anyhow!("no resolved address for remote host"))?;

    Ok(RemoteEndpoint {
        remote_addr: addr,
        host,
        port,
    })
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

pub(crate) fn request_envelope<T: Serialize>(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    payload: &T,
) -> anyhow::Result<Envelope> {
    Ok(Envelope {
        message_type,
        request_id,
        correlation_id,
        payload: serde_json::to_value(payload)?,
        error: None,
    })
}

pub(crate) fn build_command_start_envelope(
    correlation_id: Uuid,
    request_id: Uuid,
    command_type: CommandType,
    payload: CommandPayload,
) -> anyhow::Result<Envelope> {
    request_envelope(
        MessageType::CommandStart,
        request_id,
        correlation_id,
        &CommandStartRequest {
            request_id,
            command_type,
            payload,
        },
    )
}

pub(crate) async fn request_response(
    session: &web_transport_quinn::Session,
    envelope: &Envelope,
) -> anyhow::Result<Envelope> {
    request_response_with_timeout(session, envelope, resolve_deploy_stream_timeout()).await
}

pub(crate) async fn request_response_with_timeout(
    session: &web_transport_quinn::Session,
    envelope: &Envelope,
    stream_timeout: Duration,
) -> anyhow::Result<Envelope> {
    let responses = request_events_with_timeout(session, envelope, stream_timeout).await?;
    responses
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty response stream"))
}

pub(crate) async fn request_events(
    session: &web_transport_quinn::Session,
    envelope: &Envelope,
) -> anyhow::Result<Vec<Envelope>> {
    request_events_with_timeout(session, envelope, resolve_deploy_stream_timeout()).await
}

pub(crate) async fn request_events_with_timeout(
    session: &web_transport_quinn::Session,
    envelope: &Envelope,
    stream_timeout: Duration,
) -> anyhow::Result<Vec<Envelope>> {
    let payload = to_cbor(envelope)?;
    let framed = encode_frame(&payload);
    let mut attempt = 1usize;
    let response_bytes = loop {
        match request_events_once(session, &framed, stream_timeout).await {
            Ok(response_bytes) => break response_bytes,
            Err(err) => {
                let Some(backoff) = deploy_stream_retry_backoff(attempt) else {
                    return Err(err);
                };
                eprintln!(
                    "request stream attempt {attempt}/{DEPLOY_STREAM_MAX_ATTEMPTS} failed, retrying in {}ms",
                    backoff.as_millis()
                );
                tokio::time::sleep(backoff).await;
                attempt = attempt.saturating_add(1);
            }
        }
    };
    let frames = decode_frames(&response_bytes)?;
    let mut envelopes = Vec::with_capacity(frames.len());
    for frame in frames {
        envelopes.push(from_cbor::<Envelope>(&frame)?);
    }
    Ok(envelopes)
}

async fn request_events_once(
    session: &web_transport_quinn::Session,
    framed: &[u8],
    timeout_duration: Duration,
) -> anyhow::Result<Vec<u8>> {
    let (mut send, mut recv) = tokio::time::timeout(timeout_duration, session.open_bi())
        .await
        .map_err(|_| {
            anyhow!(
                "request stream open timed out after {} ms",
                timeout_duration.as_millis()
            )
        })??;
    tokio::time::timeout(timeout_duration, send.write_all(framed))
        .await
        .map_err(|_| {
            anyhow!(
                "request stream write timed out after {} ms",
                timeout_duration.as_millis()
            )
        })??;
    send.finish()?;
    tokio::time::timeout(timeout_duration, recv.read_to_end(MAX_STREAM_BYTES))
        .await
        .map_err(|_| {
            anyhow!(
                "request stream read timed out after {} ms",
                timeout_duration.as_millis()
            )
        })?
        .map_err(anyhow::Error::from)
}

fn deploy_stream_retry_backoff(attempt: usize) -> Option<Duration> {
    if attempt >= DEPLOY_STREAM_MAX_ATTEMPTS {
        return None;
    }
    DEPLOY_STREAM_RETRY_BACKOFF_MS
        .get(attempt.saturating_sub(1))
        .copied()
        .map(Duration::from_millis)
}

fn resolve_deploy_stream_timeout() -> Duration {
    let value = std::env::var("IMAGO_DEPLOY_STREAM_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_DEPLOY_STREAM_TIMEOUT_SECS);
    Duration::from_secs(value)
}

fn upload_ranges_for_prepare(
    status: ArtifactStatus,
    missing_ranges: &[ByteRange],
    artifact_size: u64,
) -> anyhow::Result<Vec<ByteRange>> {
    match status {
        ArtifactStatus::Complete => Ok(Vec::new()),
        ArtifactStatus::Missing => Ok(vec![ByteRange {
            offset: 0,
            length: artifact_size,
        }]),
        ArtifactStatus::Partial => {
            if missing_ranges.is_empty() {
                return Err(anyhow!(
                    "server reported artifact_status=partial but missing_ranges is empty"
                ));
            }
            Ok(missing_ranges.to_vec())
        }
    }
}

fn build_upload_chunk_plan(
    ranges: &[ByteRange],
    artifact_size: u64,
    chunk_size: usize,
) -> anyhow::Result<Vec<(u64, usize)>> {
    if chunk_size == 0 {
        return Err(anyhow!("chunk_size must be greater than 0"));
    }

    let chunk_size_u64 = u64::try_from(chunk_size).context("chunk_size conversion failed")?;
    let mut chunks = Vec::new();
    for range in ranges {
        if range.length == 0 {
            return Err(anyhow!("missing range length must be greater than 0"));
        }
        let range_end = range
            .offset
            .checked_add(range.length)
            .ok_or_else(|| anyhow!("missing range overflow: offset+length"))?;
        if range_end > artifact_size {
            return Err(anyhow!(
                "missing range is outside artifact size: end={} artifact_size={}",
                range_end,
                artifact_size
            ));
        }

        let mut cursor = range.offset;
        while cursor < range_end {
            let remaining = range_end - cursor;
            let chunk_len_u64 = remaining.min(chunk_size_u64);
            let chunk_len =
                usize::try_from(chunk_len_u64).context("chunk length conversion failed")?;
            chunks.push((cursor, chunk_len));
            cursor = cursor.saturating_add(chunk_len_u64);
        }
    }
    Ok(chunks)
}

async fn push_artifact_ranges(
    context: UploadRequestContext<'_>,
    artifact_path: &Path,
    artifact_size: u64,
    ranges: &[ByteRange],
    limits: UploadLimits,
) -> anyhow::Result<()> {
    let chunk_plan = build_upload_chunk_plan(ranges, artifact_size, limits.chunk_size)?;

    let mut file = tokio::fs::File::open(artifact_path)
        .await
        .with_context(|| {
            format!(
                "failed to open artifact bundle: {}",
                artifact_path.display()
            )
        })?;

    let mut uploads = JoinSet::new();
    let deploy_id = Arc::<str>::from(context.deploy_id.to_string());
    let upload_token = Arc::<str>::from(context.upload_token.to_string());

    for (offset, chunk_len) in chunk_plan {
        while uploads.len() >= limits.max_inflight_chunks {
            let completed = uploads
                .join_next()
                .await
                .ok_or_else(|| anyhow!("upload task set was unexpectedly empty"))?;
            completed.map_err(|err| anyhow!("upload task join failed: {err}"))??;
        }

        let mut chunk = vec![0u8; chunk_len];
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .with_context(|| {
                format!(
                    "failed to seek artifact bundle: {}",
                    artifact_path.display()
                )
            })?;
        file.read_exact(&mut chunk).await.with_context(|| {
            format!(
                "failed to read artifact bundle chunk: {}",
                artifact_path.display()
            )
        })?;
        let task_session = context.session.clone();
        let task_deploy_id = deploy_id.clone();
        let task_upload_token = upload_token.clone();
        let task_stream_timeout = context.stream_timeout;
        uploads.spawn(async move {
            push_single_artifact_chunk(
                task_session,
                context.correlation_id,
                task_deploy_id,
                task_upload_token,
                offset,
                chunk,
                task_stream_timeout,
            )
            .await
        });
    }

    while let Some(completed) = uploads.join_next().await {
        completed.map_err(|err| anyhow!("upload task join failed: {err}"))??;
    }

    Ok(())
}

async fn push_single_artifact_chunk(
    session: Session,
    correlation_id: Uuid,
    deploy_id: Arc<str>,
    upload_token: Arc<str>,
    offset: u64,
    chunk: Vec<u8>,
    stream_timeout: Duration,
) -> anyhow::Result<()> {
    let chunk_hash = hex::encode(Sha256::digest(&chunk));
    let chunk_b64 = base64::engine::general_purpose::STANDARD.encode(&chunk);
    let length = u64::try_from(chunk.len()).context("chunk length conversion failed")?;

    let request = request_envelope(
        MessageType::ArtifactPush,
        Uuid::new_v4(),
        correlation_id,
        &ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: deploy_id.as_ref().to_string(),
                offset,
                length,
                chunk_sha256: chunk_hash,
                upload_token: upload_token.as_ref().to_string(),
            },
            chunk_b64,
        },
    )?;

    let _ack: imago_protocol::ArtifactPushAck =
        response_payload(request_response_with_timeout(&session, &request, stream_timeout).await?)?;
    Ok(())
}

pub(crate) fn response_payload<T: serde::de::DeserializeOwned>(
    response: Envelope,
) -> anyhow::Result<T> {
    if let Some(error) = response.error {
        return Err(ServerResponseError { error }.into());
    }
    serde_json::from_value(response.payload)
        .map_err(|e| anyhow!("response payload decode failed: {e}"))
}

fn build_idempotency_key(
    name: &str,
    app_type: &str,
    target: &BTreeMap<String, String>,
    policy: &BTreeMap<String, String>,
    artifact_digest: &str,
    artifact_size: u64,
    manifest_digest: &str,
) -> String {
    let mut hasher = Sha256::new();
    update_canonical_field(&mut hasher, "name", name);
    update_canonical_field(&mut hasher, "app_type", app_type);
    update_canonical_field(&mut hasher, "artifact_digest", artifact_digest);
    update_canonical_field(&mut hasher, "artifact_size", &artifact_size.to_string());
    update_canonical_field(&mut hasher, "manifest_digest", manifest_digest);
    update_canonical_map(&mut hasher, "target", target);
    update_canonical_map(&mut hasher, "policy", policy);
    format!("deploy:{}", hex::encode(hasher.finalize()))
}

fn update_canonical_field(hasher: &mut Sha256, key: &str, value: &str) {
    hasher.update(key.as_bytes());
    hasher.update(b"\0");
    hasher.update(value.len().to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(value.as_bytes());
    hasher.update(b"\0");
}

fn update_canonical_map(hasher: &mut Sha256, key: &str, map: &BTreeMap<String, String>) {
    hasher.update(key.as_bytes());
    hasher.update(b"\0");
    for (entry_key, entry_value) in map {
        update_canonical_field(hasher, entry_key, entry_value);
    }
    hasher.update(b"\0");
}

fn normalize_target_for_protocol(target: &build::DeployTargetConfig) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    map.insert("remote".to_string(), target.remote.clone());
    if let Some(name) = &target.server_name {
        map.insert("server_name".to_string(), name.clone());
    }
    map
}

fn build_artifact_bundle_file(
    manifest: &Manifest,
    manifest_source: &Path,
    project_root: &Path,
    dependency_component_sources: &BTreeMap<String, PathBuf>,
) -> anyhow::Result<TempArtifactBundle> {
    let bundle_path = std::env::temp_dir().join(format!("imago-artifact-{}.tar", Uuid::new_v4()));
    let bundle_file = std::fs::File::create(&bundle_path).with_context(|| {
        format!(
            "failed to create artifact bundle file: {}",
            bundle_path.display()
        )
    })?;

    let mut builder = tar::Builder::new(bundle_file);
    add_file_to_tar(
        &mut builder,
        project_root.join(manifest_source),
        "manifest.json",
    )?;
    let manifest_base_dir = manifest_source.parent().unwrap_or_else(|| Path::new(""));
    let normalized_main = normalize_bundle_entry_path(&manifest.main, "manifest.main")?;
    let main_entry = normalized_tar_entry_name(&normalized_main);
    add_file_to_tar(
        &mut builder,
        project_root.join(manifest_base_dir).join(&normalized_main),
        &main_entry,
    )?;
    for asset in &manifest.assets {
        let normalized_asset = normalize_bundle_entry_path(&asset.path, "assets[].path")?;
        let asset_entry = normalized_tar_entry_name(&normalized_asset);
        add_file_to_tar(
            &mut builder,
            project_root.join(&normalized_asset),
            &asset_entry,
        )?;
    }
    for (index, dependency) in manifest.dependencies.iter().enumerate() {
        if dependency.kind != build::ManifestDependencyKind::Wasm {
            continue;
        }
        let component = dependency.component.as_ref().ok_or_else(|| {
            anyhow!("dependencies[{index}].component is required when kind=\"wasm\"")
        })?;
        let normalized_component = normalize_bundle_entry_path(
            &component.path,
            &format!("dependencies[{index}].component.path"),
        )?;
        let component_entry = normalized_tar_entry_name(&normalized_component);
        let source_path = dependency_component_sources
            .get(&dependency.name)
            .cloned()
            .unwrap_or_else(|| project_root.join(&normalized_component));
        add_file_to_tar(&mut builder, source_path, &component_entry)?;
    }
    builder.finish()?;

    Ok(TempArtifactBundle::new(bundle_path))
}

#[cfg(test)]
async fn resolve_dependency_component_sources(
    project_root: &Path,
    manifest: &Manifest,
) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    let dependency_resolver = StandardDependencyResolver;
    dependency_resolver.resolve_dependency_component_sources(project_root, &manifest.dependencies)
}

fn normalize_bundle_entry_path(raw: &str, field_name: &str) -> anyhow::Result<PathBuf> {
    if raw.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(anyhow!("{field_name} must be a relative path: {raw}"));
    }
    if raw.contains('\\') {
        return Err(anyhow!(
            "{field_name} must not contain backslash separators: {raw}"
        ));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(anyhow!("{field_name} must not be windows-prefixed: {raw}"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir => {
                return Err(anyhow!(
                    "{field_name} must not contain path traversal: {raw}"
                ));
            }
            _ => {
                return Err(anyhow!(
                    "{field_name} contains unsupported path component: {raw}"
                ));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} is invalid: {raw}"));
    }

    Ok(normalized)
}

fn normalized_tar_entry_name(path: &Path) -> String {
    path.iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn compute_file_sha256_and_size(path: &Path) -> anyhow::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to open artifact bundle: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;

    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read artifact bundle: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }

    Ok((hex::encode(hasher.finalize()), total))
}

fn add_file_to_tar<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    source: PathBuf,
    entry_name: &str,
) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(&source)
        .with_context(|| format!("failed to open file for artifact: {}", source.display()))?;
    builder
        .append_file(entry_name, &mut file)
        .with_context(|| format!("failed to append tar entry: {entry_name}"))?;
    Ok(())
}

fn load_private_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open private key: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let key = rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("failed to parse private key: {}", path.display()))?
        .ok_or_else(|| anyhow!("private key is missing: {}", path.display()))?;
    Ok(key)
}

fn build_client_raw_public_key_resolver(
    provider: Arc<CryptoProvider>,
    client_key: &PrivateKeyDer<'static>,
) -> anyhow::Result<Arc<dyn rustls::client::ResolvesClientCert>> {
    let signing_key = provider
        .key_provider
        .load_private_key(client_key.clone_key())
        .map_err(|e| anyhow!("failed to load client private key: {e}"))?;

    if signing_key.algorithm() != rustls::SignatureAlgorithm::ED25519 {
        return Err(anyhow!(
            "client private key must be ed25519 for raw public key TLS"
        ));
    }

    let spki = signing_key
        .public_key()
        .ok_or_else(|| anyhow!("failed to derive client public key from private key"))?;
    let _ = extract_ed25519_raw_public_key_from_spki(spki.as_ref())?;

    let certified_key = CertifiedKey::new(
        vec![CertificateDer::from(spki.as_ref().to_vec())],
        signing_key,
    );
    Ok(Arc::new(AlwaysResolvesClientRawPublicKeys::new(Arc::new(
        certified_key,
    ))))
}

fn extract_ed25519_raw_public_key_from_spki(spki_der: &[u8]) -> anyhow::Result<[u8; 32]> {
    if spki_der.len() != ED25519_SPKI_PREFIX.len() + 32 {
        return Err(anyhow!(
            "raw public key must be ed25519 (expected 32-byte key)"
        ));
    }
    if !spki_der.starts_with(&ED25519_SPKI_PREFIX) {
        return Err(anyhow!("raw public key must be ed25519"));
    }

    let mut raw = [0u8; 32];
    raw.copy_from_slice(&spki_der[ED25519_SPKI_PREFIX.len()..]);
    Ok(raw)
}

fn format_authority(host: &str, port: u16) -> String {
    format!(
        "{}:{}",
        format_host_for_url(host).to_ascii_lowercase(),
        port
    )
}

fn known_hosts_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("failed to resolve home directory for known_hosts"))?;
    Ok(home.join(IMAGO_DIR_NAME).join(KNOWN_HOSTS_FILE_NAME))
}

fn load_known_hosts_entries(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let content = fs::read_to_string(path)
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

        let normalized_key = normalize_ed25519_raw_key_hex(key_hex).with_context(|| {
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

fn save_known_host_entry(path: &Path, authority: &str, key_hex: &str) -> anyhow::Result<()> {
    let normalized_key = normalize_ed25519_raw_key_hex(key_hex)?;
    let mut entries = load_known_hosts_entries(path)?;

    if let Some(existing) = entries.get(authority) {
        if existing.eq_ignore_ascii_case(&normalized_key) {
            return Ok(());
        }
        return Err(anyhow!(
            "refusing to overwrite known_hosts entry for '{authority}': existing key differs; edit {} manually",
            path.display()
        ));
    }

    entries.insert(authority.to_string(), normalized_key);
    write_known_hosts_entries(path, &entries)
}

fn normalize_ed25519_raw_key_hex(value: &str) -> anyhow::Result<String> {
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

fn write_known_hosts_entries(
    path: &Path,
    entries: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        anyhow!(
            "failed to determine parent directory for known_hosts: {}",
            path.display()
        )
    })?;
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create known_hosts dir: {}", dir.display()))?;
    set_restrictive_permissions_for_dir(dir)?;

    let tmp_path = dir.join(format!(".{}.tmp-{}", KNOWN_HOSTS_FILE_NAME, Uuid::new_v4()));
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| format!("failed to open temp known_hosts: {}", tmp_path.display()))?;
        set_restrictive_permissions_for_file(&tmp_path)?;

        for (authority, key_hex) in entries {
            writeln!(file, "{authority}\t{key_hex}").with_context(|| {
                format!(
                    "failed to write temp known_hosts entries: {}",
                    tmp_path.display()
                )
            })?;
        }
        file.flush().with_context(|| {
            format!(
                "failed to flush temp known_hosts entries: {}",
                tmp_path.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!(
                "failed to sync temp known_hosts entries: {}",
                tmp_path.display()
            )
        })?;
    }

    rename_replace(&tmp_path, path)?;
    set_restrictive_permissions_for_file(path)?;
    Ok(())
}

fn rename_replace(from: &Path, to: &Path) -> anyhow::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) => {
            if to.exists() {
                fs::remove_file(to).with_context(|| {
                    format!(
                        "failed to remove existing known_hosts file: {}",
                        to.display()
                    )
                })?;
                fs::rename(from, to).with_context(|| {
                    format!("failed to replace known_hosts file: {}", to.display())
                })
            } else {
                Err(err).with_context(|| {
                    format!(
                        "failed to move known_hosts temp file from {} to {}",
                        from.display(),
                        to.display()
                    )
                })
            }
        }
    }
}

#[cfg(unix)]
fn set_restrictive_permissions_for_dir(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(IMAGO_DIR_MODE))
        .with_context(|| format!("failed to set directory permissions: {}", path.display()))
}

#[cfg(not(unix))]
fn set_restrictive_permissions_for_dir(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_restrictive_permissions_for_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(KNOWN_HOSTS_MODE))
        .with_context(|| format!("failed to set known_hosts permissions: {}", path.display()))
}

#[cfg(not(unix))]
fn set_restrictive_permissions_for_file(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_frames(value: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut offset = 0usize;

    while offset < value.len() {
        if value.len() - offset < 4 {
            return Err(anyhow!("truncated frame header"));
        }

        let len = u32::from_be_bytes(
            value[offset..offset + 4]
                .try_into()
                .map_err(|_| anyhow!("invalid frame header"))?,
        ) as usize;
        offset += 4;

        if value.len() - offset < len {
            return Err(anyhow!("truncated frame payload"));
        }

        out.push(value[offset..offset + len].to_vec());
        offset += len;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::dependency_cache;
    use imago_lockfile::{IMAGO_LOCK_VERSION, ImagoLock, ImagoLockDependency};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-deploy-tests-{test_name}-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be created");
        }
        fs::write(path, bytes).expect("file should be written");
    }

    fn write_imago_lock(root: &Path, lock: &ImagoLock) {
        let body = toml::to_string_pretty(lock).expect("lock should serialize");
        write_file(&root.join("imago.lock"), body.as_bytes());
    }

    fn sample_manifest_with_wasm_dependency(name: &str, sha256: &str) -> Manifest {
        Manifest {
            name: "svc".to_string(),
            main: "app.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![build::ManifestDependency {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                kind: build::ManifestDependencyKind::Wasm,
                wit: "file://registry/example.wit".to_string(),
                requires: vec![],
                component: Some(build::ManifestDependencyComponent {
                    path: format!("plugins/components/{sha256}.wasm"),
                    sha256: sha256.to_string(),
                }),
                capabilities: build::ManifestCapabilityPolicy::default(),
            }],
        }
    }

    #[test]
    fn maps_unknown_ca_transport_error_to_e_unauthorized() {
        let unknown_ca_code =
            quinn::TransportErrorCode::crypto(u8::from(rustls::AlertDescription::UnknownCA));
        assert!(is_certificate_alert_transport_code(unknown_ca_code));

        let err = quinn::ConnectionError::ConnectionClosed(quinn::ConnectionClose {
            error_code: unknown_ca_code,
            frame_type: None,
            reason: "unknown ca".into(),
        });

        let mapped = map_connect_connection_error(err);
        let message = mapped.to_string();
        assert!(message.contains("E_UNAUTHORIZED"));
        assert!(message.contains("transport.connect"));
    }

    #[test]
    fn maps_certificate_required_connection_closed_to_e_unauthorized() {
        let err = quinn::ConnectionError::ConnectionClosed(quinn::ConnectionClose {
            error_code: quinn::TransportErrorCode::crypto(u8::from(
                rustls::AlertDescription::CertificateRequired,
            )),
            frame_type: None,
            reason: "certificate required".into(),
        });

        let mapped = map_connect_connection_error(err);
        let message = mapped.to_string();
        assert!(message.contains("E_UNAUTHORIZED"));
        assert!(message.contains("transport.connect"));
    }

    #[test]
    fn does_not_map_non_certificate_tls_alert_to_e_unauthorized() {
        let no_alpn_code = quinn::TransportErrorCode::crypto(u8::from(
            rustls::AlertDescription::NoApplicationProtocol,
        ));
        assert!(!is_certificate_alert_transport_code(no_alpn_code));

        let err = quinn::ConnectionError::ConnectionClosed(quinn::ConnectionClose {
            error_code: no_alpn_code,
            frame_type: None,
            reason: "no application protocol".into(),
        });

        let mapped = map_connect_connection_error(err);
        let message = mapped.to_string();
        assert!(!message.contains("E_UNAUTHORIZED"));
        assert!(message.contains("failed to establish quic connection"));
    }

    #[test]
    fn maps_http_401_to_e_unauthorized() {
        let mapped = map_connect_rejection_status(
            web_transport_quinn::http::StatusCode::UNAUTHORIZED,
            "http error status: 401 Unauthorized",
        )
        .expect("http 401 should map to unauthorized");

        let message = mapped.to_string();
        assert!(message.contains("E_UNAUTHORIZED"));
        assert!(message.contains("transport.connect"));
    }

    #[test]
    fn server_identity_mismatch_error_is_normalized_as_unauthorized() {
        let err = server_identity_mismatch_error(
            "example.com:4443",
            &"aa".repeat(32),
            &"bb".repeat(32),
            Path::new("/tmp/.imago/known_hosts"),
        );
        let message = err.to_string();
        assert!(message.contains("E_UNAUTHORIZED"));
        assert!(message.contains("transport.connect"));
        assert!(message.contains("server key mismatch"));
    }

    #[test]
    fn missing_server_identity_error_is_normalized_as_unauthorized() {
        let err = missing_server_identity_error("example.com:4443");
        let message = err.to_string();
        assert!(message.contains("E_UNAUTHORIZED"));
        assert!(message.contains("transport.connect"));
        assert!(message.contains("failed to verify server raw public key"));
    }

    #[test]
    fn parse_connect_error_status_ignores_unrelated_numbers_before_status() {
        let status = parse_connect_error_status(
            "connection to 127.0.0.1:443 failed: http error status: 401 Unauthorized",
        );
        assert_eq!(
            status,
            Some(web_transport_quinn::http::StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn parse_connect_error_status_parses_403_from_http_error_prefix() {
        let status = parse_connect_error_status("http error status: 403 Forbidden");
        assert_eq!(
            status,
            Some(web_transport_quinn::http::StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn parse_connect_error_status_returns_none_without_http_error_prefix() {
        let status = parse_connect_error_status("connection to 127.0.0.1:443 failed");
        assert_eq!(status, None);
    }

    #[tokio::test]
    async fn returns_non_zero_when_build_step_fails() {
        let root =
            std::env::temp_dir().join(format!("imago-cli-deploy-run-fail-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("temp dir should be created");

        let result = run_with_project_root(DeployArgs { target: None }, &root).await;

        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.expect("stderr should be present");
        assert!(stderr.contains("failed to run build before deploy"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resolve_dependency_component_sources_reads_component_from_dependency_cache() {
        let root = new_temp_dir("dependency-cache-hit");
        let component_bytes = b"\0asmcached-component";
        let component_sha = hex::encode(Sha256::digest(component_bytes));

        write_imago_lock(
            &root,
            &ImagoLock {
                version: IMAGO_LOCK_VERSION,
                dependencies: vec![ImagoLockDependency {
                    name: "yieldspace:plugin/example".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "file://registry/example.wit".to_string(),
                    wit_registry: None,
                    wit_digest: "deadbeef".to_string(),
                    wit_path: "wit/deps/yieldspace-plugin/example".to_string(),
                    component_source: Some("file://registry/example-component.wasm".to_string()),
                    component_registry: None,
                    component_sha256: Some(component_sha.clone()),
                    resolved_at: "0".to_string(),
                }],
                binding_wits: vec![],
                wit_packages: vec![],
            },
        );

        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "yieldspace:plugin/example".to_string(),
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "file://registry/example.wit".to_string(),
            wit_registry: None,
            wit_path: "wit/deps/yieldspace-plugin/example".to_string(),
            wit_digest: "deadbeef".to_string(),
            wit_source_fingerprint: None,
            component_source: Some("file://registry/example-component.wasm".to_string()),
            component_registry: None,
            component_sha256: Some(component_sha.clone()),
            component_source_fingerprint: None,
            transitive_packages: vec![],
        };
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache metadata should be written");
        write_file(
            &dependency_cache::cache_component_path(
                &root,
                "yieldspace:plugin/example",
                &component_sha,
            ),
            component_bytes,
        );

        let manifest =
            sample_manifest_with_wasm_dependency("yieldspace:plugin/example", &component_sha);
        let sources = resolve_dependency_component_sources(&root, &manifest)
            .await
            .expect("dependency component should resolve from cache");
        let resolved = sources
            .get("yieldspace:plugin/example")
            .expect("resolved source should exist");
        assert_eq!(
            resolved,
            &dependency_cache::cache_component_path(
                &root,
                "yieldspace:plugin/example",
                &component_sha
            )
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resolve_dependency_component_sources_fails_when_dependency_cache_is_missing() {
        let root = new_temp_dir("dependency-cache-miss");
        let component_bytes = b"\0asmcached-component";
        let component_sha = hex::encode(Sha256::digest(component_bytes));

        write_imago_lock(
            &root,
            &ImagoLock {
                version: IMAGO_LOCK_VERSION,
                dependencies: vec![ImagoLockDependency {
                    name: "yieldspace:plugin/example".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "file://registry/example.wit".to_string(),
                    wit_registry: None,
                    wit_digest: "deadbeef".to_string(),
                    wit_path: "wit/deps/yieldspace-plugin/example".to_string(),
                    component_source: Some("file://registry/example-component.wasm".to_string()),
                    component_registry: None,
                    component_sha256: Some(component_sha.clone()),
                    resolved_at: "0".to_string(),
                }],
                binding_wits: vec![],
                wit_packages: vec![],
            },
        );

        let manifest =
            sample_manifest_with_wasm_dependency("yieldspace:plugin/example", &component_sha);
        let err = resolve_dependency_component_sources(&root, &manifest)
            .await
            .expect_err("missing dependency cache must fail");
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains(".imago/deps"),
            "unexpected error: {err:#}"
        );
        assert!(
            err_chain.contains("imago update"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn parse_remote_endpoint_supports_ipv6_literals() {
        let bare_ipv6 = parse_remote_endpoint("::1")
            .await
            .expect("bare ipv6 should parse");
        assert_eq!(bare_ipv6.host, "::1");
        assert_eq!(bare_ipv6.port, 4443);

        let bracketed = parse_remote_endpoint("[::1]:4443")
            .await
            .expect("bracketed ipv6 should parse");
        assert_eq!(bracketed.host, "::1");
        assert_eq!(bracketed.port, 4443);

        let https_ipv6 = parse_remote_endpoint("https://[::1]:4443")
            .await
            .expect("https ipv6 should parse");
        assert_eq!(https_ipv6.host, "::1");
        assert_eq!(https_ipv6.port, 4443);
    }

    #[test]
    fn command_start_envelope_uses_same_request_id_for_header_and_payload() {
        let request_id = Uuid::new_v4();
        let envelope = build_command_start_envelope(
            Uuid::new_v4(),
            request_id,
            CommandType::Deploy,
            CommandPayload::Deploy(DeployCommandPayload {
                deploy_id: "deploy-1".to_string(),
                expected_current_release: "any".to_string(),
                restart_policy: "never".to_string(),
                auto_rollback: true,
            }),
        )
        .expect("envelope should be created");

        assert_eq!(envelope.request_id, request_id);
        let payload: CommandStartRequest =
            serde_json::from_value(envelope.payload).expect("payload should deserialize");
        assert_eq!(payload.request_id, request_id);
    }

    #[test]
    fn parse_upload_limits_uses_hello_limits_values() {
        let response = HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod-test".to_string(),
            features: vec![],
            limits: BTreeMap::from([
                ("chunk_size".to_string(), "2048".to_string()),
                ("max_inflight_chunks".to_string(), "4".to_string()),
                ("deploy_stream_timeout_secs".to_string(), "12".to_string()),
            ]),
        };

        let limits = parse_upload_limits(&response).expect("limits should parse");
        assert_eq!(
            limits,
            UploadLimits {
                chunk_size: 2048,
                max_inflight_chunks: 4,
                deploy_stream_timeout: Duration::from_secs(12),
            }
        );
    }

    #[test]
    fn parse_upload_limits_rejects_zero_values() {
        let response = HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod-test".to_string(),
            features: vec![],
            limits: BTreeMap::from([("chunk_size".to_string(), "0".to_string())]),
        };

        let err = parse_upload_limits(&response).expect_err("zero chunk_size must fail");
        assert!(err.to_string().contains("chunk_size"));
    }

    fn sample_structured_error(code: ErrorCode, retryable: bool) -> StructuredError {
        StructuredError {
            code,
            message: "error".to_string(),
            retryable,
            stage: "upload".to_string(),
            details: BTreeMap::new(),
        }
    }

    fn sample_server_error(code: ErrorCode, retryable: bool) -> anyhow::Error {
        ServerResponseError {
            error: sample_structured_error(code, retryable),
        }
        .into()
    }

    #[test]
    fn client_bind_candidates_include_ipv6_then_ipv4_fallback() {
        let candidates = client_bind_candidates();
        assert_eq!(
            candidates[0],
            "[::]:0".parse::<SocketAddr>().expect("valid address")
        );
        assert_eq!(
            candidates[1],
            "0.0.0.0:0".parse::<SocketAddr>().expect("valid address")
        );
    }

    #[test]
    fn upload_ranges_for_partial_requires_missing_ranges() {
        let err = upload_ranges_for_prepare(ArtifactStatus::Partial, &[], 1024)
            .expect_err("partial without missing_ranges must fail");
        assert!(err.to_string().contains("missing_ranges"));
    }

    #[test]
    fn build_upload_chunk_plan_uses_requested_ranges_only() {
        let ranges = vec![
            ByteRange {
                offset: 0,
                length: 4,
            },
            ByteRange {
                offset: 10,
                length: 3,
            },
        ];
        let plan = build_upload_chunk_plan(&ranges, 32, 2).expect("chunk plan should build");
        assert_eq!(plan, vec![(0, 2), (2, 2), (10, 2), (12, 1)]);
    }

    #[test]
    fn build_upload_chunk_plan_rejects_out_of_bounds_range() {
        let ranges = vec![ByteRange {
            offset: 8,
            length: 4,
        }];
        let err = build_upload_chunk_plan(&ranges, 10, 2)
            .expect_err("range outside artifact size must fail");
        assert!(err.to_string().contains("outside artifact size"));
    }

    #[test]
    fn idempotency_key_is_stable_for_same_payload() {
        let target = BTreeMap::from([
            ("remote".to_string(), "127.0.0.1:4443".to_string()),
            ("server_name".to_string(), "imagod.local".to_string()),
        ]);
        let policy = BTreeMap::from([("rollout".to_string(), "safe".to_string())]);

        let first = build_idempotency_key(
            "svc",
            "cli",
            &target,
            &policy,
            "digest-a",
            1024,
            "manifest-a",
        );
        let second = build_idempotency_key(
            "svc",
            "cli",
            &target,
            &policy,
            "digest-a",
            1024,
            "manifest-a",
        );

        assert_eq!(first, second);
        assert!(first.starts_with("deploy:"));
        assert_eq!(first.len(), "deploy:".len() + 64);
    }

    #[test]
    fn idempotency_key_changes_when_target_changes() {
        let key_a = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::from([("remote".to_string(), "127.0.0.1:4443".to_string())]),
            &BTreeMap::new(),
            "digest-a",
            1024,
            "manifest-a",
        );
        let key_b = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::from([("remote".to_string(), "127.0.0.1:5555".to_string())]),
            &BTreeMap::new(),
            "digest-a",
            1024,
            "manifest-a",
        );

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn idempotency_key_changes_when_policy_changes() {
        let key_a = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::new(),
            &BTreeMap::from([("rollout".to_string(), "safe".to_string())]),
            "digest-a",
            1024,
            "manifest-a",
        );
        let key_b = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::new(),
            &BTreeMap::from([("rollout".to_string(), "fast".to_string())]),
            "digest-a",
            1024,
            "manifest-a",
        );

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn retry_classification_retries_busy_or_internal() {
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Busy,
            false
        )));
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Busy,
            true
        )));
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Internal,
            true
        )));
    }

    #[test]
    fn retry_classification_does_not_retry_bad_request_or_unauthorized() {
        assert!(!should_retry_upload_error(&sample_server_error(
            ErrorCode::BadRequest,
            true
        )));
        assert!(!should_retry_upload_error(&sample_server_error(
            ErrorCode::Unauthorized,
            true
        )));
    }

    #[test]
    fn retry_backoff_is_bounded_and_increasing() {
        assert_eq!(retry_backoff_duration(1), Duration::from_millis(250));
        assert_eq!(retry_backoff_duration(2), Duration::from_millis(500));
        assert_eq!(retry_backoff_duration(3), Duration::from_millis(1000));
        assert_eq!(retry_backoff_duration(4), Duration::from_millis(1000));
    }

    #[test]
    fn deploy_stream_retry_backoff_is_bounded() {
        assert_eq!(
            deploy_stream_retry_backoff(1),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            deploy_stream_retry_backoff(2),
            Some(Duration::from_millis(250))
        );
        assert_eq!(deploy_stream_retry_backoff(3), None);
    }

    #[test]
    fn retry_classification_does_not_retry_when_server_marks_non_retryable() {
        assert!(!should_retry_upload_error(&sample_server_error(
            ErrorCode::Internal,
            false
        )));
    }

    #[test]
    fn retry_classification_retries_busy_even_when_server_marks_non_retryable() {
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Busy,
            false
        )));
    }

    #[test]
    fn retry_classification_does_not_retry_unstructured_unauthorized_error() {
        let err = anyhow!(
            "server error: public key authentication failed (E_UNAUTHORIZED) at transport.connect"
        );
        assert!(!should_retry_upload_error(&err));
    }

    #[test]
    fn retry_classification_does_not_retry_commit_not_verified_error() {
        let err: anyhow::Error = CommitNotVerifiedError.into();
        assert!(!should_retry_upload_error(&err));
    }

    #[test]
    fn truncate_log_message_never_exceeds_max_chars() {
        assert_eq!(truncate_log_message("abc", 3), "abc");
        assert_eq!(truncate_log_message("abcdef", 6), "abcdef");
        assert_eq!(truncate_log_message("abcdef", 5), "ab...");
        assert_eq!(truncate_log_message("abcdef", 3), "...");
        assert_eq!(truncate_log_message("abcdef", 2), "..");
        assert_eq!(truncate_log_message("abcdef", 0), "");
    }

    #[test]
    fn format_retry_log_message_reports_failed_attempt() {
        let message = format_retry_log_message(1, 4, Duration::from_millis(250), "E_BUSY");
        assert!(message.contains("upload attempt 1/4 failed"));
        assert!(message.contains("retrying in 250ms"));
        assert!(message.contains("reason=E_BUSY"));
    }

    #[test]
    fn extracts_ed25519_raw_public_key_from_spki() {
        let mut spki = ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(&[0x11; 32]);
        let key =
            extract_ed25519_raw_public_key_from_spki(&spki).expect("ed25519 spki should parse");
        assert_eq!(key, [0x11; 32]);
    }

    #[test]
    fn tofu_verifier_supports_only_ed25519_verify_scheme() {
        let verifier =
            TofuServerCertVerifier::new(web_transport_quinn::crypto::default_provider(), None);
        let schemes =
            rustls::client::danger::ServerCertVerifier::supported_verify_schemes(&verifier);
        assert_eq!(schemes, vec![SignatureScheme::ED25519]);
    }

    #[test]
    fn rejects_non_ed25519_spki() {
        let mut spki = ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(&[0x11; 32]);
        spki[0] = 0x31;
        let err =
            extract_ed25519_raw_public_key_from_spki(&spki).expect_err("invalid spki should fail");
        assert!(err.to_string().contains("ed25519"));
    }

    #[test]
    fn format_authority_brackets_ipv6_and_lowercases() {
        assert_eq!(
            format_authority("EXAMPLE.COM", 4443),
            "example.com:4443".to_string()
        );
        assert_eq!(format_authority("::1", 4443), "[::1]:4443".to_string());
    }

    #[test]
    fn known_hosts_round_trip_and_conflict_detection() {
        let root = new_temp_dir("known-hosts");
        let path = root.join("known_hosts");
        let authority = "example.com:4443";
        let key_hex_upper = "AA".repeat(32);

        save_known_host_entry(&path, authority, &key_hex_upper).expect("entry should be written");
        let entries = load_known_hosts_entries(&path).expect("entries should load");
        assert_eq!(
            entries.get(authority),
            Some(&"aa".repeat(32)),
            "stored key should be normalized to lowercase"
        );

        save_known_host_entry(&path, authority, &"aa".repeat(32))
            .expect("same key should be idempotent");
        let err = save_known_host_entry(&path, authority, &"bb".repeat(32))
            .expect_err("conflicting key must be rejected");
        assert!(err.to_string().contains("refusing to overwrite"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalize_bundle_entry_path_rejects_unsafe_values() {
        assert!(normalize_bundle_entry_path("../evil.wasm", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("/etc/passwd", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("C:\\evil.wasm", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("..\\evil.wasm", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("app/main.wasm", "manifest.main").is_ok());
    }

    #[test]
    fn build_artifact_bundle_file_includes_hashed_main_wasm() {
        let root = std::env::temp_dir().join(format!("imago-cli-bundle-hashed-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("build")).expect("build dir should be created");
        fs::write(root.join("build/manifest.json"), "{}").expect("manifest source should exist");

        let hashed_main =
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef-svc.wasm";
        fs::write(root.join("build").join(hashed_main), b"wasm").expect("hashed main should exist");

        let manifest = Manifest {
            name: "svc".to_string(),
            main: hashed_main.to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![],
        };

        let bundle = build_artifact_bundle_file(
            &manifest,
            Path::new("build/manifest.json"),
            &root,
            &BTreeMap::new(),
        )
        .expect("bundle should be created");

        let file = std::fs::File::open(bundle.path()).expect("bundle file should open");
        let mut archive = tar::Archive::new(file);
        let mut names = Vec::new();
        for entry in archive.entries().expect("tar entries should be readable") {
            let entry = entry.expect("tar entry should read");
            let path = entry.path().expect("entry path should parse");
            names.push(path.to_string_lossy().to_string());
        }

        assert!(names.contains(&"manifest.json".to_string()));
        assert!(names.contains(&hashed_main.to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_artifact_bundle_file_rejects_unsafe_manifest_main() {
        let root = std::env::temp_dir().join(format!("imago-cli-bundle-main-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("build")).expect("build dir should be created");
        fs::write(root.join("build/manifest.json"), "{}").expect("manifest source should exist");

        let manifest = Manifest {
            name: "svc".to_string(),
            main: "../evil.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![],
        };

        let err = build_artifact_bundle_file(
            &manifest,
            Path::new("build/manifest.json"),
            &root,
            &BTreeMap::new(),
        )
        .expect_err("unsafe manifest.main should be rejected");
        assert!(err.to_string().contains("manifest.main"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_artifact_bundle_file_rejects_unsafe_asset_path() {
        let root = std::env::temp_dir().join(format!("imago-cli-bundle-asset-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("build")).expect("build dir should be created");
        fs::write(root.join("build/manifest.json"), "{}").expect("manifest source should exist");
        fs::write(root.join("build/main.wasm"), b"00").expect("main wasm should exist");

        let manifest = Manifest {
            name: "svc".to_string(),
            main: "main.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![ManifestAsset {
                path: "../secret.txt".to_string(),
            }],
            dependencies: vec![],
        };

        let err = build_artifact_bundle_file(
            &manifest,
            Path::new("build/manifest.json"),
            &root,
            &BTreeMap::new(),
        )
        .expect_err("unsafe asset path should be rejected");
        assert!(err.to_string().contains("assets[].path"));

        let _ = fs::remove_dir_all(root);
    }
}
