use std::{
    collections::BTreeMap,
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use imago_protocol::{
    ErrorCode, HelloNegotiateRequest, HelloNegotiateResponse, MessageType, PROTOCOL_VERSION,
    ProtocolEnvelope, RpcInvokeRequest, RpcInvokeResponse, RpcInvokeTargetService,
    SUPPORTED_PROTOCOL_VERSION_RANGE, Validate, from_cbor, to_cbor,
};
use imagod_common::ImagodError;
use imagod_config::{ImagodConfig, upsert_tls_known_public_key};
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
use semver::{Version, VersionReq};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::net::lookup_host;
use url::{Host, Url};
use uuid::Uuid;
use web_transport_quinn::{Session, proto::ConnectRequest};

const STAGE_REMOTE_RPC: &str = "service.control.remote_rpc";
const DEFAULT_RPC_PORT: u16 = 4443;
const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
const DATAGRAM_BUFFER_BYTES: usize = 1024 * 1024;
const STREAM_TIMEOUT: Duration = Duration::from_secs(15);
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

type Envelope = ProtocolEnvelope<Value>;

#[derive(Debug, Clone)]
pub(super) struct RemoteAuthority {
    key: String,
    host: String,
    host_for_url: String,
    port: u16,
}

#[derive(Debug, Clone)]
struct RemoteConnection {
    authority: RemoteAuthority,
}

#[derive(Debug, Default)]
pub(super) struct RemoteRpcManager {
    config_path: PathBuf,
    by_runner: BTreeMap<String, BTreeMap<String, RemoteConnection>>,
}

impl RemoteRpcManager {
    pub(super) fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            by_runner: BTreeMap::new(),
        }
    }

    pub(super) fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub(super) async fn probe_remote_authority(
        config_path: &Path,
        authority: &str,
    ) -> Result<RemoteAuthority, ImagodError> {
        let authority = parse_rpc_authority(authority)?;
        let session = connect_remote_session(config_path, &authority).await?;
        session.close(0, b"rpc.connect probe complete");
        Ok(authority)
    }

    pub(super) fn insert_connection(
        &mut self,
        runner_id: &str,
        authority: RemoteAuthority,
    ) -> String {
        let connection_id = Uuid::new_v4().to_string();
        self.by_runner
            .entry(runner_id.to_string())
            .or_default()
            .insert(connection_id.clone(), RemoteConnection { authority });
        connection_id
    }

    pub(super) fn connection_for(
        &self,
        runner_id: &str,
        connection_id: &str,
    ) -> Option<RemoteAuthority> {
        self.by_runner
            .get(runner_id)
            .and_then(|connections| connections.get(connection_id))
            .map(|connection| connection.authority.clone())
    }

    pub(super) async fn invoke_with_authority(
        config_path: &Path,
        authority: &RemoteAuthority,
        target_service: &str,
        interface_id: &str,
        function: &str,
        args_cbor: &[u8],
    ) -> Result<Vec<u8>, ImagodError> {
        invoke_remote_authority(
            config_path,
            authority,
            target_service,
            interface_id,
            function,
            args_cbor,
        )
        .await
    }

    pub(super) fn disconnect(&mut self, runner_id: &str, connection_id: &str) -> bool {
        let Some(connections) = self.by_runner.get_mut(runner_id) else {
            return false;
        };
        let removed = connections.remove(connection_id).is_some();
        if connections.is_empty() {
            self.by_runner.remove(runner_id);
        }
        removed
    }
}

fn remote_error(code: ErrorCode, message: impl Into<String>) -> ImagodError {
    ImagodError::new(code, STAGE_REMOTE_RPC, message.into())
}

fn hello_rejection_message(response: &HelloNegotiateResponse) -> String {
    if let Some(announcement) = response.compatibility_announcement.as_deref()
        && !announcement.trim().is_empty()
    {
        return announcement.to_string();
    }

    format!(
        "hello.negotiate was rejected by remote imagod (server_protocol_version={}, supported_protocol_version_range={})",
        response.server_protocol_version, response.supported_protocol_version_range
    )
}

fn ensure_server_protocol_version_supported(
    response: &HelloNegotiateResponse,
) -> Result<(), ImagodError> {
    let supported_range = VersionReq::parse(SUPPORTED_PROTOCOL_VERSION_RANGE).map_err(|err| {
        remote_error(
            ErrorCode::Internal,
            format!(
                "invalid client supported protocol range '{}': {err}",
                SUPPORTED_PROTOCOL_VERSION_RANGE
            ),
        )
    })?;
    let server_protocol_version =
        Version::parse(&response.server_protocol_version).map_err(|err| {
            remote_error(
                ErrorCode::BadRequest,
                format!(
                    "server_protocol_version '{}' is not valid semver: {err}",
                    response.server_protocol_version
                ),
            )
        })?;
    if supported_range.matches(&server_protocol_version) {
        return Ok(());
    }

    Err(remote_error(
        ErrorCode::BadRequest,
        format!(
            "server protocol version '{}' is not supported by this client (client supports '{}')",
            response.server_protocol_version, SUPPORTED_PROTOCOL_VERSION_RANGE
        ),
    ))
}

async fn invoke_remote_authority(
    config_path: &Path,
    authority: &RemoteAuthority,
    target_service: &str,
    interface_id: &str,
    function: &str,
    args_cbor: &[u8],
) -> Result<Vec<u8>, ImagodError> {
    let session = connect_remote_session(config_path, authority).await?;
    let correlation_id = Uuid::new_v4();

    negotiate_hello(&session, correlation_id).await?;
    let invoke_request = request_envelope(
        MessageType::RpcInvoke,
        Uuid::new_v4(),
        correlation_id,
        &RpcInvokeRequest {
            interface_id: interface_id.to_string(),
            function: function.to_string(),
            args_cbor: args_cbor.to_vec(),
            target_service: RpcInvokeTargetService {
                name: target_service.to_string(),
            },
        },
    )?;
    let invoke_response: RpcInvokeResponse =
        response_payload(request_response(&session, &invoke_request).await?)?;
    session.close(0, b"rpc.invoke complete");

    invoke_response
        .validate()
        .map_err(|err| remote_error(ErrorCode::BadRequest, err.to_string()))?;
    if let Some(err) = invoke_response.error {
        return Err(ImagodError::new(err.code, err.stage, err.message));
    }
    invoke_response.result_cbor.ok_or_else(|| {
        remote_error(
            ErrorCode::Internal,
            "rpc.invoke response missing result_cbor",
        )
    })
}

async fn negotiate_hello(session: &Session, correlation_id: Uuid) -> Result<(), ImagodError> {
    let hello_request = request_envelope(
        MessageType::HelloNegotiate,
        Uuid::new_v4(),
        correlation_id,
        &HelloNegotiateRequest {
            client_version: PROTOCOL_VERSION.to_string(),
            required_features: vec!["rpc.invoke".to_string()],
        },
    )?;
    let hello_response: HelloNegotiateResponse =
        response_payload(request_response(session, &hello_request).await?)?;

    if !hello_response.accepted {
        return Err(remote_error(
            ErrorCode::BadRequest,
            hello_rejection_message(&hello_response),
        ));
    }
    ensure_server_protocol_version_supported(&hello_response)?;
    if !hello_response
        .features
        .iter()
        .any(|feature| feature == "rpc.invoke")
    {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "remote imagod does not support rpc.invoke",
        ));
    }
    Ok(())
}

fn request_envelope<T: Serialize>(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    payload: &T,
) -> Result<Envelope, ImagodError> {
    let payload = serde_json::to_value(payload).map_err(|err| {
        remote_error(ErrorCode::Internal, format!("payload encode failed: {err}"))
    })?;
    Ok(Envelope {
        message_type,
        request_id,
        correlation_id,
        payload,
        error: None,
    })
}

fn response_payload<T: DeserializeOwned + Validate>(response: Envelope) -> Result<T, ImagodError> {
    if let Some(err) = response.error {
        return Err(ImagodError::new(err.code, err.stage, err.message));
    }
    let payload: T = serde_json::from_value(response.payload).map_err(|err| {
        remote_error(
            ErrorCode::BadRequest,
            format!("response payload decode failed: {err}"),
        )
    })?;
    payload
        .validate()
        .map_err(|err| remote_error(ErrorCode::BadRequest, err.to_string()))?;
    Ok(payload)
}

async fn request_response(session: &Session, envelope: &Envelope) -> Result<Envelope, ImagodError> {
    let payload = to_cbor(envelope)
        .map_err(|err| remote_error(ErrorCode::Internal, format!("frame encode failed: {err}")))?;
    let framed = encode_frame(&payload);

    let (mut send, mut recv) = tokio::time::timeout(STREAM_TIMEOUT, session.open_bi())
        .await
        .map_err(|_| remote_error(ErrorCode::OperationTimeout, "open_bi timed out"))?
        .map_err(|err| remote_error(ErrorCode::Internal, format!("open_bi failed: {err}")))?;
    tokio::time::timeout(STREAM_TIMEOUT, send.write_all(&framed))
        .await
        .map_err(|_| remote_error(ErrorCode::OperationTimeout, "stream write timed out"))?
        .map_err(|err| remote_error(ErrorCode::Internal, format!("stream write failed: {err}")))?;
    send.finish()
        .map_err(|err| remote_error(ErrorCode::Internal, format!("stream finish failed: {err}")))?;

    let response_bytes = tokio::time::timeout(STREAM_TIMEOUT, recv.read_to_end(MAX_STREAM_BYTES))
        .await
        .map_err(|_| remote_error(ErrorCode::OperationTimeout, "stream read timed out"))?
        .map_err(|err| remote_error(ErrorCode::Internal, format!("stream read failed: {err}")))?;
    let frames = decode_frames(&response_bytes)?;
    let Some(first) = frames.first() else {
        return Err(remote_error(
            ErrorCode::Internal,
            "remote response stream was empty",
        ));
    };
    from_cbor(first).map_err(|err| {
        remote_error(
            ErrorCode::BadRequest,
            format!("response envelope decode failed: {err}"),
        )
    })
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn decode_frames(value: &[u8]) -> Result<Vec<Vec<u8>>, ImagodError> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset < value.len() {
        if value.len() - offset < 4 {
            return Err(remote_error(
                ErrorCode::BadRequest,
                "truncated frame header",
            ));
        }
        let len = u32::from_be_bytes(
            value[offset..offset + 4]
                .try_into()
                .map_err(|_| remote_error(ErrorCode::BadRequest, "invalid frame header"))?,
        ) as usize;
        offset += 4;
        if value.len() - offset < len {
            return Err(remote_error(
                ErrorCode::BadRequest,
                "truncated frame payload",
            ));
        }
        out.push(value[offset..offset + len].to_vec());
        offset += len;
    }
    Ok(out)
}

async fn connect_remote_session(
    config_path: &Path,
    authority: &RemoteAuthority,
) -> Result<Session, ImagodError> {
    let config = ImagodConfig::load(config_path)?;
    let expected_key_hex = config.tls.known_public_keys.get(&authority.key).cloned();
    let client_key_path = resolve_client_key_path(config_path, &config.tls.server_key);
    let client_key = load_private_key(&client_key_path)?;

    let provider = web_transport_quinn::crypto::default_provider();
    let verifier = Arc::new(TofuServerCertVerifier::new(
        provider.clone(),
        expected_key_hex,
    ));
    let client_resolver = build_client_raw_public_key_resolver(provider.clone(), &client_key)?;
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|err| {
            remote_error(
                ErrorCode::Internal,
                format!("tls version setup failed: {err}"),
            )
        })?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_client_cert_resolver(client_resolver);
    tls.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];

    let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|err| {
        remote_error(ErrorCode::Internal, format!("quic tls setup failed: {err}"))
    })?;
    let mut quic_config = quinn::ClientConfig::new(Arc::new(quic_tls));
    let mut transport = quinn::TransportConfig::default();
    transport.datagram_send_buffer_size(DATAGRAM_BUFFER_BYTES);
    transport.datagram_receive_buffer_size(Some(DATAGRAM_BUFFER_BYTES));
    quic_config.transport_config(Arc::new(transport));

    let endpoint = create_client_endpoint()?;
    let remote_addr = resolve_remote_socket_addr(&authority.host, authority.port).await?;
    let connecting = endpoint
        .connect_with(quic_config, remote_addr, &authority.host)
        .map_err(|err| {
            remote_error(
                ErrorCode::Internal,
                format!("quic connect start failed: {err}"),
            )
        })?;
    let connection = connecting
        .await
        .map_err(|err| remote_error(ErrorCode::Internal, format!("quic connect failed: {err}")))?;

    let request_url = Url::parse(&format!(
        "https://{}:{}/",
        authority.host_for_url, authority.port
    ))
    .map_err(|err| {
        remote_error(
            ErrorCode::BadRequest,
            format!("request URL parse failed: {err}"),
        )
    })?;
    let request = ConnectRequest::new(request_url);
    let session = Session::connect(connection, request).await.map_err(|err| {
        remote_error(
            ErrorCode::Internal,
            format!("webtransport connect failed: {err}"),
        )
    })?;

    match verifier.take_observed_status() {
        Some(ServerIdentityStatus::Matched { .. }) => Ok(session),
        Some(ServerIdentityStatus::Unknown { presented_key_hex }) => {
            if let Err(err) =
                upsert_tls_known_public_key(config_path, &authority.key, &presented_key_hex)
            {
                session.close(0, b"failed to persist known_public_keys entry");
                return Err(err);
            }
            Ok(session)
        }
        Some(ServerIdentityStatus::Mismatch {
            expected_key_hex,
            presented_key_hex,
        }) => {
            session.close(0, b"server raw public key mismatch");
            Err(remote_error(
                ErrorCode::Unauthorized,
                format!(
                    "server key mismatch for authority '{}': expected {}, got {}",
                    authority.key, expected_key_hex, presented_key_hex
                ),
            ))
        }
        None => {
            session.close(0, b"missing server identity verification");
            Err(remote_error(
                ErrorCode::Unauthorized,
                format!(
                    "failed to verify server raw public key for authority '{}'",
                    authority.key
                ),
            ))
        }
    }
}

fn resolve_client_key_path(config_path: &Path, configured: &Path) -> PathBuf {
    let path = configured.to_path_buf();
    if path.is_absolute() {
        return path;
    }
    config_path
        .parent()
        .map(|parent| parent.join(path.clone()))
        .unwrap_or(path)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, ImagodError> {
    let file = std::fs::File::open(path).map_err(|err| {
        remote_error(
            ErrorCode::Internal,
            format!("private key open failed: {err}"),
        )
    })?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|err| {
            remote_error(
                ErrorCode::BadRequest,
                format!("private key parse failed: {err}"),
            )
        })?
        .ok_or_else(|| remote_error(ErrorCode::BadRequest, "private key is missing"))
}

fn build_client_raw_public_key_resolver(
    provider: Arc<CryptoProvider>,
    client_key: &PrivateKeyDer<'static>,
) -> Result<Arc<dyn rustls::client::ResolvesClientCert>, ImagodError> {
    let signing_key = provider
        .key_provider
        .load_private_key(client_key.clone_key())
        .map_err(|err| {
            remote_error(
                ErrorCode::BadRequest,
                format!("client key load failed: {err}"),
            )
        })?;
    if signing_key.algorithm() != rustls::SignatureAlgorithm::ED25519 {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "client private key must be ed25519 for raw public key TLS",
        ));
    }
    let spki = signing_key.public_key().ok_or_else(|| {
        remote_error(
            ErrorCode::BadRequest,
            "failed to derive public key from private key",
        )
    })?;
    let _ = extract_ed25519_raw_public_key_from_spki(spki.as_ref())?;
    let certified_key = CertifiedKey::new(
        vec![CertificateDer::from(spki.as_ref().to_vec())],
        signing_key,
    );
    Ok(Arc::new(AlwaysResolvesClientRawPublicKeys::new(Arc::new(
        certified_key,
    ))))
}

fn extract_ed25519_raw_public_key_from_spki(spki_der: &[u8]) -> Result<[u8; 32], ImagodError> {
    if spki_der.len() != ED25519_SPKI_PREFIX.len() + 32 {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "raw public key must be ed25519 (expected 32-byte key)",
        ));
    }
    if !spki_der.starts_with(&ED25519_SPKI_PREFIX) {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "raw public key must be ed25519",
        ));
    }
    let mut raw = [0u8; 32];
    raw.copy_from_slice(&spki_der[ED25519_SPKI_PREFIX.len()..]);
    Ok(raw)
}

async fn resolve_remote_socket_addr(host: &str, port: u16) -> Result<SocketAddr, ImagodError> {
    let mut addrs = lookup_host((host, port)).await.map_err(|err| {
        remote_error(
            ErrorCode::BadRequest,
            format!("failed to resolve remote host {host}:{port}: {err}"),
        )
    })?;
    addrs.next().ok_or_else(|| {
        remote_error(
            ErrorCode::BadRequest,
            format!("no resolved address for remote host {host}:{port}"),
        )
    })
}

fn create_client_endpoint() -> Result<quinn::Endpoint, ImagodError> {
    let candidates = [
        "[::]:0"
            .parse::<SocketAddr>()
            .expect("valid ipv6 wildcard address"),
        "0.0.0.0:0"
            .parse::<SocketAddr>()
            .expect("valid ipv4 wildcard address"),
    ];
    let mut last_error = None;
    for bind_addr in candidates {
        match quinn::Endpoint::client(bind_addr) {
            Ok(endpoint) => return Ok(endpoint),
            Err(err) => {
                last_error = Some(remote_error(
                    ErrorCode::Internal,
                    format!("failed to bind client endpoint on {bind_addr}: {err}"),
                ));
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| remote_error(ErrorCode::Internal, "failed to bind client endpoint")))
}

fn parse_rpc_authority(authority: &str) -> Result<RemoteAuthority, ImagodError> {
    let parsed = Url::parse(authority).map_err(|err| {
        remote_error(
            ErrorCode::BadRequest,
            format!("invalid rpc authority: {err}"),
        )
    })?;
    if parsed.scheme() != "rpc" {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "rpc authority must use rpc:// scheme",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "rpc authority must not contain userinfo",
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "rpc authority must not contain query or fragment",
        ));
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "rpc authority must not contain a path",
        ));
    }
    let host = match parsed
        .host()
        .ok_or_else(|| remote_error(ErrorCode::BadRequest, "rpc authority host is required"))?
    {
        Host::Ipv6(ip) => ip.to_string(),
        Host::Ipv4(ip) => ip.to_string(),
        Host::Domain(domain) => domain.to_ascii_lowercase(),
    };
    let port = parsed.port().unwrap_or(DEFAULT_RPC_PORT);
    if port == 0 {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "rpc authority port must be greater than zero",
        ));
    }
    let host_for_url = format_host_for_url(&host);
    let key = format!("rpc://{}:{}", host_for_url.to_ascii_lowercase(), port);
    Ok(RemoteAuthority {
        key,
        host,
        host_for_url,
        port,
    })
}

fn format_host_for_url(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

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

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        time::Duration,
    };

    use imago_protocol::{HelloNegotiateResponse, StructuredError, Validate, ValidationError};
    use rustls::client::danger::ServerCertVerifier;
    use serde::{Deserialize, Serialize};

    use super::*;

    fn test_server_key_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/local-imagod-plugin-hello/certs/server.key")
    }

    fn ed25519_spki_from_raw(raw: [u8; 32]) -> Vec<u8> {
        let mut spki = ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(&raw);
        spki
    }

    #[test]
    fn given_rpc_authority_variants__when_parse_rpc_authority__then_normalization_and_rejection_follow_contract()
     {
        let parsed =
            parse_rpc_authority("rpc://Node-A").expect("authority without port should parse");
        assert_eq!(parsed.host, "node-a");
        assert_eq!(parsed.host_for_url, "node-a");
        assert_eq!(parsed.port, DEFAULT_RPC_PORT);
        assert_eq!(parsed.key, "rpc://node-a:4443");

        let parsed_ipv6 =
            parse_rpc_authority("rpc://[2001:db8::1]:7443").expect("ipv6 authority should parse");
        assert_eq!(parsed_ipv6.host, "2001:db8::1");
        assert_eq!(parsed_ipv6.host_for_url, "[2001:db8::1]");
        assert_eq!(parsed_ipv6.port, 7443);
        assert_eq!(parsed_ipv6.key, "rpc://[2001:db8::1]:7443");

        let invalid_cases = [
            ("https://node-a:4443", "must use rpc:// scheme"),
            ("rpc://user@node-a:4443", "must not contain userinfo"),
            ("rpc://node-a:4443/path", "must not contain a path"),
            (
                "rpc://node-a:4443?x=1",
                "must not contain query or fragment",
            ),
            ("rpc://node-a:0", "port must be greater than zero"),
            ("rpc://:4443", "invalid rpc authority"),
        ];
        for (authority, expected_fragment) in invalid_cases {
            let err =
                parse_rpc_authority(authority).expect_err("invalid authority should be rejected");
            assert_eq!(err.code, ErrorCode::BadRequest);
            assert_eq!(err.stage, STAGE_REMOTE_RPC);
            assert!(
                err.message.contains(expected_fragment),
                "authority={authority} unexpected message: {}",
                err.message
            );
        }
    }

    #[test]
    fn given_host_string__when_format_host_for_url__then_ipv6_is_bracketed() {
        assert_eq!(format_host_for_url("node-a"), "node-a");
        assert_eq!(format_host_for_url("[::1]"), "[::1]");
        assert_eq!(format_host_for_url("::1"), "[::1]");
    }

    #[test]
    fn given_binary_frames__when_decode_frames__then_round_trip_and_truncation_errors_are_stable() {
        let first = b"hello".to_vec();
        let second = vec![0x01, 0x02, 0x03];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&encode_frame(&first));
        bytes.extend_from_slice(&encode_frame(&second));

        let frames = decode_frames(&bytes).expect("decode should succeed");
        assert_eq!(frames, vec![first, second]);

        let header_err =
            decode_frames(&[0x00, 0x00, 0x00]).expect_err("truncated header should fail");
        assert_eq!(header_err.code, ErrorCode::BadRequest);
        assert_eq!(header_err.stage, STAGE_REMOTE_RPC);
        assert_eq!(header_err.message, "truncated frame header");

        let payload_err = decode_frames(&[0x00, 0x00, 0x00, 0x05, 0x41, 0x42])
            .expect_err("truncated payload should fail");
        assert_eq!(payload_err.code, ErrorCode::BadRequest);
        assert_eq!(payload_err.stage, STAGE_REMOTE_RPC);
        assert_eq!(payload_err.message, "truncated frame payload");
    }

    #[test]
    fn given_hello_response__when_hello_rejection_message__then_announcement_is_preferred() {
        let with_announcement = HelloNegotiateResponse {
            accepted: false,
            server_version: "imagod/test".to_string(),
            server_protocol_version: "0.1.0".to_string(),
            supported_protocol_version_range: "^0.1.0".to_string(),
            compatibility_announcement: Some("upgrade required".to_string()),
            features: vec![],
            limits: BTreeMap::new(),
        };
        assert_eq!(
            hello_rejection_message(&with_announcement),
            "upgrade required"
        );

        let without_announcement = HelloNegotiateResponse {
            compatibility_announcement: None,
            ..with_announcement
        };
        let fallback = hello_rejection_message(&without_announcement);
        assert!(fallback.contains("hello.negotiate was rejected"));
        assert!(fallback.contains("supported_protocol_version_range"));
    }

    #[test]
    fn given_server_protocol_versions__when_ensure_server_protocol_version_supported__then_supported_rejected_and_invalid_cases_are_mapped()
     {
        let supported = HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod/test".to_string(),
            server_protocol_version: "0.1.0".to_string(),
            supported_protocol_version_range: "^0.1.0".to_string(),
            compatibility_announcement: None,
            features: vec!["rpc.invoke".to_string()],
            limits: BTreeMap::new(),
        };
        ensure_server_protocol_version_supported(&supported)
            .expect("supported protocol should pass");

        let unsupported = HelloNegotiateResponse {
            server_protocol_version: "9.9.9".to_string(),
            ..supported.clone()
        };
        let err = ensure_server_protocol_version_supported(&unsupported)
            .expect_err("unsupported protocol should fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(err.message.contains("is not supported by this client"));

        let invalid = HelloNegotiateResponse {
            server_protocol_version: "not-semver".to_string(),
            ..supported
        };
        let err = ensure_server_protocol_version_supported(&invalid)
            .expect_err("invalid semver should fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(err.message.contains("is not valid semver"));
    }

    #[test]
    fn given_config_and_key_paths__when_resolve_client_key_path__then_relative_joins_and_absolute_is_preserved()
     {
        let config_path = PathBuf::from("/tmp/imago/imagod.toml");
        let relative = resolve_client_key_path(&config_path, &PathBuf::from("certs/server.key"));
        assert_eq!(relative, PathBuf::from("/tmp/imago/certs/server.key"));

        let absolute =
            resolve_client_key_path(&config_path, &PathBuf::from("/etc/imago/server.key"));
        assert_eq!(absolute, PathBuf::from("/etc/imago/server.key"));
    }

    #[test]
    fn given_spki_data__when_extract_ed25519_raw_public_key_from_spki__then_valid_and_invalid_cases_are_checked()
     {
        let raw = [0x42u8; 32];
        let spki = ed25519_spki_from_raw(raw);
        let parsed = extract_ed25519_raw_public_key_from_spki(&spki)
            .expect("valid ed25519 spki should pass");
        assert_eq!(parsed, raw);

        let short = vec![0u8; ED25519_SPKI_PREFIX.len() + 31];
        let err =
            extract_ed25519_raw_public_key_from_spki(&short).expect_err("short spki should fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(err.message.contains("expected 32-byte key"));

        let mut wrong_prefix = ed25519_spki_from_raw(raw);
        wrong_prefix[8] = 0x71;
        let err = extract_ed25519_raw_public_key_from_spki(&wrong_prefix)
            .expect_err("wrong prefix should fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(err.message.contains("must be ed25519"));
    }

    #[test]
    fn given_server_cert_and_expected_key__when_verify_server_cert__then_tofu_status_is_recorded() {
        let raw = [0x24u8; 32];
        let cert = CertificateDer::from(ed25519_spki_from_raw(raw));
        let now = UnixTime::since_unix_epoch(Duration::from_secs(0));
        let server_name = ServerName::try_from("localhost").expect("server name should parse");

        let matched_verifier = TofuServerCertVerifier::new(
            web_transport_quinn::crypto::default_provider(),
            Some(hex::encode(raw)),
        );
        matched_verifier
            .verify_server_cert(&cert, &[], &server_name, &[], now)
            .expect("verify should succeed");
        assert!(matches!(
            matched_verifier.take_observed_status(),
            Some(ServerIdentityStatus::Matched { .. })
        ));

        let unknown_verifier =
            TofuServerCertVerifier::new(web_transport_quinn::crypto::default_provider(), None);
        unknown_verifier
            .verify_server_cert(&cert, &[], &server_name, &[], now)
            .expect("verify should succeed");
        assert!(matches!(
            unknown_verifier.take_observed_status(),
            Some(ServerIdentityStatus::Unknown { .. })
        ));

        let mismatch_verifier = TofuServerCertVerifier::new(
            web_transport_quinn::crypto::default_provider(),
            Some(hex::encode([0x99u8; 32])),
        );
        mismatch_verifier
            .verify_server_cert(&cert, &[], &server_name, &[], now)
            .expect("verify should succeed");
        assert!(matches!(
            mismatch_verifier.take_observed_status(),
            Some(ServerIdentityStatus::Mismatch { .. })
        ));
    }

    #[test]
    fn given_private_key_sources__when_load_private_key_and_build_resolver__then_success_and_error_cases_are_mapped()
     {
        let valid_path = test_server_key_path();
        let key = load_private_key(&valid_path).expect("sample key should load");

        let resolver = build_client_raw_public_key_resolver(
            web_transport_quinn::crypto::default_provider(),
            &key,
        );
        assert!(resolver.is_ok(), "ed25519 key should build resolver");

        let missing_err = load_private_key(Path::new("/no/such/private-key.pem"))
            .expect_err("missing should fail");
        assert_eq!(missing_err.code, ErrorCode::Internal);
        assert_eq!(missing_err.stage, STAGE_REMOTE_RPC);
        assert!(missing_err.message.contains("private key open failed"));

        let temp_path = std::env::temp_dir().join(format!(
            "imagod-remote-rpc-invalid-key-{}.pem",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &temp_path,
            "-----BEGIN PRIVATE KEY-----\n%%%invalid-base64%%%\n-----END PRIVATE KEY-----\n",
        )
        .expect("temp key write should succeed");
        let parse_err = load_private_key(&temp_path).expect_err("invalid key should fail");
        assert_eq!(parse_err.code, ErrorCode::BadRequest);
        assert_eq!(parse_err.stage, STAGE_REMOTE_RPC);
        assert!(
            parse_err.message.contains("private key parse failed")
                || parse_err.message.contains("private key is missing"),
            "unexpected parse error: {}",
            parse_err.message
        );
        let _ = std::fs::remove_file(temp_path);
    }

    #[test]
    fn given_bind_candidates__when_create_client_endpoint__then_endpoint_is_created() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build");
        runtime.block_on(async {
            let endpoint = create_client_endpoint().expect("client endpoint should be creatable");
            drop(endpoint);
        });
    }

    #[test]
    fn given_payload_serialization_cases__when_request_envelope__then_internal_encode_errors_are_mapped()
     {
        #[derive(Serialize)]
        struct RequestPayload {
            name: &'static str,
            value: u32,
        }
        struct FailingPayload;
        impl Serialize for FailingPayload {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("serialize failed"))
            }
        }

        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let envelope = request_envelope(
            MessageType::RpcInvoke,
            request_id,
            correlation_id,
            &RequestPayload {
                name: "svc-a",
                value: 42,
            },
        )
        .expect("request envelope should be created");
        assert_eq!(envelope.message_type, MessageType::RpcInvoke);
        assert_eq!(envelope.request_id, request_id);
        assert_eq!(envelope.correlation_id, correlation_id);
        assert_eq!(envelope.payload["name"], "svc-a");
        assert_eq!(envelope.payload["value"], 42);

        let err = request_envelope(
            MessageType::RpcInvoke,
            Uuid::new_v4(),
            Uuid::new_v4(),
            &FailingPayload,
        )
        .expect_err("serialize failure should be mapped");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(err.message.contains("payload encode failed"));
    }

    #[test]
    fn given_response_payload_cases__when_response_payload__then_error_decode_and_validation_paths_are_mapped()
     {
        #[derive(Debug, Deserialize, PartialEq)]
        struct ResponsePayload {
            service: String,
            count: u32,
        }
        impl Validate for ResponsePayload {
            fn validate(&self) -> Result<(), ValidationError> {
                Ok(())
            }
        }
        #[derive(Debug, Deserialize)]
        struct ValidateFailPayload {
            service: String,
        }
        impl Validate for ValidateFailPayload {
            fn validate(&self) -> Result<(), ValidationError> {
                Err(ValidationError::invalid(
                    "service",
                    "forced validation failure",
                ))
            }
        }

        let structured = StructuredError {
            code: ErrorCode::Unauthorized,
            stage: "rpc.invoke".to_string(),
            message: "denied".to_string(),
            retryable: false,
            details: BTreeMap::new(),
        };
        let envelope_with_error = Envelope {
            message_type: MessageType::RpcInvoke,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::Value::Null,
            error: Some(structured),
        };
        let err = response_payload::<ResponsePayload>(envelope_with_error)
            .expect_err("response.error should be surfaced");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "rpc.invoke");
        assert_eq!(err.message, "denied");

        let decode_error_envelope = Envelope {
            message_type: MessageType::RpcInvoke,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::json!({"service":"svc-a","count":"NaN"}),
            error: None,
        };
        let err = response_payload::<ResponsePayload>(decode_error_envelope)
            .expect_err("invalid payload shape should fail decode");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(err.message.contains("response payload decode failed"));

        let validate_error_envelope = Envelope {
            message_type: MessageType::RpcInvoke,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::json!({"service":"svc-a"}),
            error: None,
        };
        let err = response_payload::<ValidateFailPayload>(validate_error_envelope)
            .expect_err("validation failure should be mapped");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(err.message.contains("forced validation failure"));
    }

    #[tokio::test]
    async fn given_host_resolution_inputs__when_resolve_remote_socket_addr__then_success_and_failure_are_mapped()
     {
        let localhost = resolve_remote_socket_addr("localhost", 443)
            .await
            .expect("localhost should resolve");
        assert_eq!(localhost.port(), 443);

        let err = resolve_remote_socket_addr("does-not-exist.invalid", 4443)
            .await
            .expect_err("invalid host should fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(
            err.message.contains("failed to resolve remote host")
                || err.message.contains("no resolved address"),
            "unexpected resolution message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn given_unresolvable_authority__when_connect_remote_session__then_resolution_error_is_returned()
     {
        let test_root =
            std::env::temp_dir().join(format!("imagod-remote-rpc-connect-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&test_root).expect("test root create should succeed");
        let config_path = test_root.join("imagod.toml");
        imagod_config::load_or_create_default(&config_path)
            .expect("default config should be generated");

        let authority = parse_rpc_authority("rpc://does-not-exist.invalid:4443")
            .expect("authority should parse");
        let err = connect_remote_session(&config_path, &authority)
            .await
            .expect_err("resolution failure should be reported");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_REMOTE_RPC);
        assert!(
            err.message.contains("failed to resolve remote host")
                || err.message.contains("no resolved address"),
            "unexpected connect error: {}",
            err.message
        );

        let _ = std::fs::remove_dir_all(test_root);
    }

    #[test]
    fn given_connection_registry__when_insert_connection_and_disconnect__then_lookup_behaves_as_expected()
     {
        let mut manager = RemoteRpcManager::new(PathBuf::from("/tmp/imagod.toml"));
        let authority = RemoteAuthority {
            key: "rpc://node-a:4443".to_string(),
            host: "node-a".to_string(),
            host_for_url: "node-a".to_string(),
            port: 4443,
        };
        let connection_id = manager.insert_connection("runner-a", authority.clone());
        let resolved = manager
            .connection_for("runner-a", &connection_id)
            .expect("connection should resolve");
        assert_eq!(resolved.key, authority.key);

        assert!(manager.disconnect("runner-a", &connection_id));
        assert!(manager.connection_for("runner-a", &connection_id).is_none());
        assert!(!manager.disconnect("runner-a", "missing"));
    }
}
