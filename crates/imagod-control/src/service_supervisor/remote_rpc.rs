use std::{
    collections::BTreeMap,
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use imago_protocol::{
    ErrorCode, HelloNegotiateRequest, HelloNegotiateResponse, MessageType, ProtocolEnvelope,
    RpcInvokeRequest, RpcInvokeResponse, RpcInvokeTargetService, Validate, from_cbor, to_cbor,
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
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::net::lookup_host;
use url::Url;
use uuid::Uuid;
use web_transport_quinn::{Session, proto::ConnectRequest};

const STAGE_REMOTE_RPC: &str = "service.control.remote_rpc";
const DEFAULT_RPC_PORT: u16 = 4443;
const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
const DATAGRAM_BUFFER_BYTES: usize = 1024 * 1024;
const STREAM_TIMEOUT: Duration = Duration::from_secs(15);
const COMPATIBILITY_DATE: &str = "2026-02-10";
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

type Envelope = ProtocolEnvelope<Value>;

#[derive(Debug, Clone)]
struct RemoteAuthority {
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

    pub(super) async fn connect(
        &mut self,
        runner_id: &str,
        authority: &str,
    ) -> Result<String, ImagodError> {
        let authority = parse_rpc_authority(authority)?;
        let session = connect_remote_session(&self.config_path, &authority).await?;
        session.close(0, b"rpc.connect probe complete");

        let connection_id = Uuid::new_v4().to_string();
        self.by_runner
            .entry(runner_id.to_string())
            .or_default()
            .insert(connection_id.clone(), RemoteConnection { authority });
        Ok(connection_id)
    }

    pub(super) async fn invoke(
        &self,
        runner_id: &str,
        connection_id: &str,
        target_service: &str,
        interface_id: &str,
        function: &str,
        args_cbor: &[u8],
    ) -> Result<Vec<u8>, ImagodError> {
        let Some(connections) = self.by_runner.get(runner_id) else {
            return Err(remote_error(
                ErrorCode::NotFound,
                format!("rpc connection '{connection_id}' is not available"),
            ));
        };
        let Some(connection) = connections.get(connection_id) else {
            return Err(remote_error(
                ErrorCode::NotFound,
                format!("rpc connection '{connection_id}' is not available"),
            ));
        };
        invoke_remote_authority(
            &self.config_path,
            &connection.authority,
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
            compatibility_date: COMPATIBILITY_DATE.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            required_features: vec!["rpc.invoke".to_string()],
        },
    )?;
    let hello_response: HelloNegotiateResponse =
        response_payload(request_response(session, &hello_request).await?)?;

    if !hello_response.accepted {
        return Err(remote_error(
            ErrorCode::BadRequest,
            "hello.negotiate was rejected by remote imagod",
        ));
    }
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
    let host = parsed
        .host_str()
        .ok_or_else(|| remote_error(ErrorCode::BadRequest, "rpc authority host is required"))?
        .to_ascii_lowercase();
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
