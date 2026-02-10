use std::{
    collections::BTreeMap,
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, anyhow};
use base64::Engine;
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushRequest, ArtifactStatus,
    CommandEvent, CommandStartRequest, CommandStartResponse, CommandType, DeployCommandPayload,
    DeployPrepareRequest, DeployPrepareResponse, Envelope, EventType, HelloNegotiateRequest,
    HelloNegotiateResponse, MESSAGE_ARTIFACT_COMMIT, MESSAGE_ARTIFACT_PUSH, MESSAGE_COMMAND_EVENT,
    MESSAGE_COMMAND_START, MESSAGE_DEPLOY_PREPARE, MESSAGE_HELLO_NEGOTIATE, Manifest,
    decode_frames, encode_frame, from_cbor, to_cbor,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::net::lookup_host;
use url::Url;
use uuid::Uuid;
use web_transport_quinn::{Session, proto::ConnectRequest};

use crate::cli::DeployArgs;

const CHUNK_SIZE: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stderr: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImagoToml {
    #[serde(default)]
    target: BTreeMap<String, TargetConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct TargetConfig {
    remote: String,
    server_name: Option<String>,
    ca_cert: PathBuf,
    client_cert: PathBuf,
    client_key: PathBuf,
}

pub fn run(args: DeployArgs) -> CommandResult {
    match run_inner(args) {
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

fn run_inner(args: DeployArgs) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;
    runtime.block_on(run_async(args))
}

async fn run_async(args: DeployArgs) -> anyhow::Result<()> {
    let manifest_path = resolve_manifest_path(args.env.as_deref());
    let manifest_bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("failed to read manifest: {}", manifest_path.display()))?;
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("failed to parse manifest json")?;

    let imago_toml = load_imago_toml(Path::new("imago.toml"))?;
    let target_name = args.target.unwrap_or_else(|| "default".to_string());
    let target = imago_toml
        .target
        .get(&target_name)
        .ok_or_else(|| anyhow!("target '{}' is not defined in imago.toml", target_name))?
        .clone();

    let artifact = build_artifact_bundle(&manifest, &manifest_path, Path::new("."))?;
    let artifact_digest = hex::encode(Sha256::digest(&artifact));
    let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));

    let session = connect_target(&target).await?;
    let correlation_id = Uuid::new_v4().to_string();

    let hello_request_id = Uuid::new_v4().to_string();
    let hello = Envelope::request(
        MESSAGE_HELLO_NEGOTIATE,
        hello_request_id.clone(),
        correlation_id.clone(),
        &HelloNegotiateRequest {
            protocol_draft: "imago-mvp-v1".to_string(),
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

    let prepare_request_id = Uuid::new_v4().to_string();
    let prepare = Envelope::request(
        MESSAGE_DEPLOY_PREPARE,
        prepare_request_id,
        correlation_id.clone(),
        &DeployPrepareRequest {
            name: manifest.name.clone(),
            service_type: manifest.service_type,
            target: normalize_target_for_protocol(&target),
            artifact_digest: artifact_digest.clone(),
            artifact_size: artifact.len() as u64,
            manifest_digest: manifest_digest.clone(),
            idempotency_key: format!("{}:{}:{}", manifest.name, artifact_digest, manifest_digest),
            policy: BTreeMap::new(),
        },
    )?;
    let prepare_response: DeployPrepareResponse =
        response_payload(request_response(&session, &prepare).await?)?;

    if prepare_response.artifact_status != ArtifactStatus::Complete {
        push_artifact_chunks(
            &session,
            &correlation_id,
            &prepare_response.deploy_id,
            &prepare_response.upload_token,
            &artifact,
        )
        .await?;
    }

    let commit_request_id = Uuid::new_v4().to_string();
    let commit = Envelope::request(
        MESSAGE_ARTIFACT_COMMIT,
        commit_request_id,
        correlation_id.clone(),
        &ArtifactCommitRequest {
            deploy_id: prepare_response.deploy_id.clone(),
            artifact_digest: artifact_digest.clone(),
            artifact_size: artifact.len() as u64,
            manifest_digest: manifest_digest.clone(),
        },
    )?;
    let commit_response: ArtifactCommitResponse =
        response_payload(request_response(&session, &commit).await?)?;
    if !commit_response.verified {
        return Err(anyhow!("artifact.commit returned verified=false"));
    }

    let command_request_id = Uuid::new_v4().to_string();
    let command = Envelope::request(
        MESSAGE_COMMAND_START,
        command_request_id.clone(),
        correlation_id.clone(),
        &CommandStartRequest {
            request_id: command_request_id.clone(),
            command_type: CommandType::Deploy,
            payload: serde_json::to_value(DeployCommandPayload {
                deploy_id: prepare_response.deploy_id.clone(),
                expected_current_release: None,
                restart_policy: None,
                auto_rollback: true,
            })?,
        },
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
        if envelope.message_type != MESSAGE_COMMAND_EVENT {
            continue;
        }
        let event: CommandEvent = response_payload(envelope.clone())?;
        if let Some(stage) = &event.stage {
            eprintln!("event={:?} stage={}", event.event_type, stage);
        }
        if matches!(
            event.event_type,
            EventType::Succeeded | EventType::Failed | EventType::Canceled
        ) {
            terminal = Some(event);
            break;
        }
    }

    let terminal =
        terminal.ok_or_else(|| anyhow!("command.event terminal event was not received"))?;
    match terminal.event_type {
        EventType::Succeeded => Ok(()),
        EventType::Failed => {
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
        EventType::Canceled => Err(anyhow!("deploy was canceled")),
        _ => Err(anyhow!("unexpected terminal event")),
    }
}

async fn connect_target(target: &TargetConfig) -> anyhow::Result<Session> {
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
    let endpoint = quinn::Endpoint::client("[::]:0".parse::<SocketAddr>()?)?;

    let endpoint_info = parse_remote_endpoint(&target.remote).await?;
    let sni = target
        .server_name
        .as_deref()
        .unwrap_or(&endpoint_info.host)
        .to_string();
    let connection = endpoint
        .connect_with(quic_config, endpoint_info.remote_addr, &sni)?
        .await?;

    let request_url = Url::parse(&format!("https://{}:{}/", sni, endpoint_info.port))
        .context("failed to build webtransport request URL")?;
    let request = ConnectRequest::new(request_url);
    Session::connect(connection, request)
        .await
        .map_err(Into::into)
}

struct RemoteEndpoint {
    remote_addr: SocketAddr,
    host: String,
    port: u16,
}

async fn parse_remote_endpoint(remote: &str) -> anyhow::Result<RemoteEndpoint> {
    if remote.starts_with("https://") {
        let url = Url::parse(remote).context("remote URL parse failed")?;
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("remote URL host is missing"))?
            .to_string();
        let port = url.port().unwrap_or(4443);
        let mut addresses = lookup_host((host.clone(), port))
            .await
            .with_context(|| format!("failed to resolve remote host: {host}:{port}"))?;
        let addr = addresses
            .next()
            .ok_or_else(|| anyhow!("no resolved address for remote host"))?;
        return Ok(RemoteEndpoint {
            remote_addr: addr,
            host,
            port,
        });
    }

    let (host, port) = if let Some((host, port)) = remote.rsplit_once(':') {
        let parsed_port = port
            .parse::<u16>()
            .with_context(|| format!("invalid remote port in '{remote}'"))?;
        (host.to_string(), parsed_port)
    } else {
        (remote.to_string(), 4443)
    };

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

async fn push_artifact_chunks(
    session: &web_transport_quinn::Session,
    correlation_id: &str,
    deploy_id: &str,
    upload_token: &str,
    artifact: &[u8],
) -> anyhow::Result<()> {
    let mut offset = 0usize;
    while offset < artifact.len() {
        let end = (offset + CHUNK_SIZE).min(artifact.len());
        let chunk = &artifact[offset..end];
        let chunk_hash = hex::encode(Sha256::digest(chunk));
        let chunk_b64 = base64::engine::general_purpose::STANDARD.encode(chunk);

        let request = Envelope::request(
            MESSAGE_ARTIFACT_PUSH,
            Uuid::new_v4().to_string(),
            correlation_id.to_string(),
            &ArtifactPushRequest {
                deploy_id: deploy_id.to_string(),
                offset: offset as u64,
                length: chunk.len() as u64,
                chunk_sha256: chunk_hash,
                upload_token: upload_token.to_string(),
                chunk_b64,
            },
        )?;

        let _ = request_response(session, &request).await?;
        offset = end;
    }
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
    response
        .payload_as()
        .map_err(|e| anyhow!("response payload decode failed: {e}"))
}

fn resolve_manifest_path(env: Option<&str>) -> PathBuf {
    if let Some(env_name) = env {
        let env_path = PathBuf::from(format!("build/manifest.{env_name}.json"));
        if env_path.exists() {
            return env_path;
        }
    }
    PathBuf::from("build/manifest.json")
}

fn load_imago_toml(path: &Path) -> anyhow::Result<ImagoToml> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&raw).context("failed to parse imago.toml")
}

fn normalize_target_for_protocol(target: &TargetConfig) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    map.insert("remote".to_string(), target.remote.clone());
    if let Some(name) = &target.server_name {
        map.insert("server_name".to_string(), name.clone());
    }
    map
}

fn build_artifact_bundle(
    manifest: &Manifest,
    manifest_source: &Path,
    project_root: &Path,
) -> anyhow::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buffer);

        add_file_to_tar(
            &mut builder,
            project_root.join(manifest_source),
            "manifest.json",
        )?;

        add_file_to_tar(
            &mut builder,
            project_root.join(&manifest.main),
            &manifest.main,
        )?;

        for asset in &manifest.assets {
            add_file_to_tar(&mut builder, project_root.join(&asset.path), &asset.path)?;
        }

        builder.finish()?;
    }
    Ok(buffer)
}

fn add_file_to_tar(
    builder: &mut tar::Builder<&mut Vec<u8>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_non_zero_when_manifest_missing() {
        let result = run(DeployArgs {
            env: None,
            target: None,
        });
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.is_some());
    }

    #[test]
    fn resolves_env_manifest_when_exists() {
        let path = resolve_manifest_path(Some("prod"));
        if Path::new("build/manifest.prod.json").exists() {
            assert_eq!(path, PathBuf::from("build/manifest.prod.json"));
        } else {
            assert_eq!(path, PathBuf::from("build/manifest.json"));
        }
    }
}
