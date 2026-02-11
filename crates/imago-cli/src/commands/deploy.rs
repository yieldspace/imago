use std::{
    collections::BTreeMap,
    io::{BufReader, Read},
    net::{IpAddr, SocketAddr},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, anyhow};
use base64::Engine;
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushChunkHeader, ArtifactPushRequest,
    ArtifactStatus, ByteRange, CommandEvent, CommandEventType, CommandPayload, CommandStartRequest,
    CommandStartResponse, CommandType, DeployCommandPayload, DeployPrepareRequest,
    DeployPrepareResponse, HelloNegotiateRequest, HelloNegotiateResponse, MessageType,
    ProtocolEnvelope, from_cbor, to_cbor,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
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
    commands::{CommandResult, build},
};

const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
const COMPATIBILITY_DATE: &str = "2026-02-10";
const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
const DEFAULT_MAX_INFLIGHT_CHUNKS: usize = 16;
const TRANSPORT_CONNECT_STAGE: &str = "transport.connect";

type Envelope = ProtocolEnvelope<Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UploadLimits {
    chunk_size: usize,
    max_inflight_chunks: usize,
}

#[derive(Clone, Copy)]
struct UploadRequestContext<'a> {
    session: &'a web_transport_quinn::Session,
    correlation_id: Uuid,
    deploy_id: &'a str,
    upload_token: &'a str,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    main: String,
    #[serde(rename = "type")]
    app_type: String,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
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

pub fn run(args: DeployArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(args: DeployArgs, project_root: &Path) -> CommandResult {
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

fn run_inner(args: DeployArgs, project_root: &Path) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;
    runtime.block_on(run_async(args, project_root))
}

async fn run_async(args: DeployArgs, project_root: &Path) -> anyhow::Result<()> {
    let target_name = args
        .target
        .clone()
        .unwrap_or_else(|| build::default_target_name().to_string());
    let build_output = build::build_project(args.env.as_deref(), &target_name, project_root)
        .context("failed to run build before deploy")?;

    let manifest_path = build_output.manifest_path;
    let manifest_bytes = build_output.manifest_bytes;
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("failed to parse manifest json")?;

    let target = build_output
        .target
        .require_deploy_credentials()
        .context("target settings are invalid for deploy")?;

    let artifact = build_artifact_bundle_file(&manifest, &manifest_path, project_root)?;
    let (artifact_digest, artifact_size) = compute_file_sha256_and_size(artifact.path())?;
    let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));

    let session = connect_target(&target).await?;
    let correlation_id = Uuid::new_v4();

    let hello = request_envelope(
        MessageType::HelloNegotiate,
        Uuid::new_v4(),
        correlation_id,
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
        correlation_id,
        &DeployPrepareRequest {
            name: manifest.name.clone(),
            app_type: manifest.app_type.clone(),
            target: normalize_target_for_protocol(&target),
            artifact_digest: artifact_digest.clone(),
            artifact_size,
            manifest_digest: manifest_digest.clone(),
            idempotency_key: format!("{}:{}:{}", manifest.name, artifact_digest, manifest_digest),
            policy: BTreeMap::new(),
        },
    )?;
    let prepare_response: DeployPrepareResponse =
        response_payload(request_response(&session, &prepare).await?)?;

    let upload_ranges = upload_ranges_for_prepare(
        prepare_response.artifact_status,
        &prepare_response.missing_ranges,
        artifact_size,
    )?;
    if !upload_ranges.is_empty() {
        let upload_context = UploadRequestContext {
            session: &session,
            correlation_id,
            deploy_id: &prepare_response.deploy_id,
            upload_token: &prepare_response.upload_token,
        };
        push_artifact_ranges(
            upload_context,
            artifact.path(),
            artifact_size,
            &upload_ranges,
            upload_limits,
        )
        .await?;
    }

    let commit = request_envelope(
        MessageType::ArtifactCommit,
        Uuid::new_v4(),
        correlation_id,
        &ArtifactCommitRequest {
            deploy_id: prepare_response.deploy_id.clone(),
            artifact_digest: artifact_digest.clone(),
            artifact_size,
            manifest_digest: manifest_digest.clone(),
        },
    )?;
    let commit_response: ArtifactCommitResponse =
        response_payload(request_response(&session, &commit).await?)?;
    if !commit_response.verified {
        return Err(anyhow!("artifact.commit returned verified=false"));
    }

    let command_request_id = Uuid::new_v4();
    let command = build_command_start_envelope(
        correlation_id,
        command_request_id,
        CommandType::Deploy,
        CommandPayload::Deploy(DeployCommandPayload {
            deploy_id: prepare_response.deploy_id.clone(),
            expected_current_release: "any".to_string(),
            restart_policy: "never".to_string(),
            auto_rollback: true,
        }),
    )?;

    let responses = request_events(&session, &command).await?;
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

async fn connect_target(target: &build::DeployTargetConfig) -> anyhow::Result<Session> {
    let ca_chain = load_certs(&target.ca_cert)?;
    let client_chain = load_certs(&target.client_cert)?;
    let client_key = load_private_key(&target.client_key)?;

    let mut roots = rustls::RootCertStore::empty();
    for cert in ca_chain {
        roots
            .add(cert)
            .map_err(|e| anyhow!("failed to add CA certificate: {e}"))?;
    }

    let mut tls = rustls::ClientConfig::builder_with_provider(
        web_transport_quinn::crypto::default_provider(),
    )
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_root_certificates(roots)
    .with_client_auth_cert(client_chain, client_key)?;
    tls.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];

    let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let quic_config = quinn::ClientConfig::new(Arc::new(quic_tls));
    let endpoint = create_client_endpoint()?;

    let endpoint_info = parse_remote_endpoint(&target.remote).await?;
    let configured_host = target.server_name.as_deref().unwrap_or(&endpoint_info.host);
    let sni = configured_host.to_string();
    let connecting = endpoint
        .connect_with(quic_config, endpoint_info.remote_addr, &sni)
        .map_err(|e| anyhow!("failed to start quic connection: {e}"))?;
    let connection = connecting.await.map_err(map_connect_connection_error)?;

    let request_host = format_host_for_url(configured_host);
    let request_url = Url::parse(&format!("https://{}:{}/", request_host, endpoint_info.port))
        .context("failed to build webtransport request URL")?;
    let request = ConnectRequest::new(request_url);
    Session::connect(connection, request)
        .await
        .map_err(map_webtransport_client_error)
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
        "server error: certificate authentication failed (E_UNAUTHORIZED) at {TRANSPORT_CONNECT_STAGE}: {source}"
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

    Ok(UploadLimits {
        chunk_size,
        max_inflight_chunks,
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

fn request_envelope<T: Serialize>(
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

fn build_command_start_envelope(
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

async fn request_response(
    session: &web_transport_quinn::Session,
    envelope: &Envelope,
) -> anyhow::Result<Envelope> {
    let responses = request_events(session, envelope).await?;
    responses
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty response stream"))
}

async fn request_events(
    session: &web_transport_quinn::Session,
    envelope: &Envelope,
) -> anyhow::Result<Vec<Envelope>> {
    let payload = to_cbor(envelope)?;
    let framed = encode_frame(&payload);

    let (mut send, mut recv) = session.open_bi().await?;
    send.write_all(&framed).await?;
    send.finish()?;

    let response_bytes = recv.read_to_end(MAX_STREAM_BYTES).await?;
    let frames = decode_frames(&response_bytes)?;
    let mut envelopes = Vec::with_capacity(frames.len());
    for frame in frames {
        envelopes.push(from_cbor::<Envelope>(&frame)?);
    }
    Ok(envelopes)
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
        uploads.spawn(async move {
            push_single_artifact_chunk(
                task_session,
                context.correlation_id,
                task_deploy_id,
                task_upload_token,
                offset,
                chunk,
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
        response_payload(request_response(&session, &request).await?)?;
    Ok(())
}

fn response_payload<T: serde::de::DeserializeOwned>(response: Envelope) -> anyhow::Result<T> {
    if let Some(error) = response.error {
        return Err(anyhow!(
            "server error: {} ({:?}) at {}",
            error.message,
            error.code,
            error.stage
        ));
    }
    serde_json::from_value(response.payload)
        .map_err(|e| anyhow!("response payload decode failed: {e}"))
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
    builder.finish()?;

    Ok(TempArtifactBundle::new(bundle_path))
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

fn load_certs(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open cert: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse certs: {}", path.display()))?;
    if certs.is_empty() {
        return Err(anyhow!("certificate file is empty: {}", path.display()));
    }
    Ok(certs)
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
    use std::fs;

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

    #[test]
    fn returns_non_zero_when_build_step_fails() {
        let root =
            std::env::temp_dir().join(format!("imago-cli-deploy-run-fail-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("temp dir should be created");

        let result = run_with_project_root(
            DeployArgs {
                env: None,
                target: None,
            },
            &root,
        );

        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.expect("stderr should be present");
        assert!(stderr.contains("failed to run build before deploy"));

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
            ]),
        };

        let limits = parse_upload_limits(&response).expect("limits should parse");
        assert_eq!(
            limits,
            UploadLimits {
                chunk_size: 2048,
                max_inflight_chunks: 4
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
        };

        let bundle = build_artifact_bundle_file(&manifest, Path::new("build/manifest.json"), &root)
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
        };

        let err = build_artifact_bundle_file(&manifest, Path::new("build/manifest.json"), &root)
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
        };

        let err = build_artifact_bundle_file(&manifest, Path::new("build/manifest.json"), &root)
            .expect_err("unsafe asset path should be rejected");
        assert!(err.to_string().contains("assets[].path"));

        let _ = fs::remove_dir_all(root);
    }
}
