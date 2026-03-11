use std::{
    any::Any,
    fmt::Write,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use imago_protocol::{ErrorCode, HelloNegotiateRequest, MessageType, Validate};
use imagod_common::ImagodError;
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
#[cfg(unix)]
use tokio::net::UnixStream;
use uuid::Uuid;
use web_transport_quinn::{RecvStream, SendStream, Session};

use super::{
    DynamicClientRole, MAX_STREAM_BYTES, ProtocolHandler, STREAM_READ_TIMEOUT_SECS,
    envelope_io::{
        error_envelope, finish_stream, parse_single_request_envelope, payload_as,
        response_message_type_for_request, write_envelope,
    },
    resolve_dynamic_client_role,
};

const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];
const LOGS_STREAM_FEATURE: &str = "logs.stream";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionAuthContext {
    pub(crate) role: DynamicClientRole,
    pub(crate) public_key_hex: String,
}

impl SessionAuthContext {
    #[cfg(unix)]
    fn local_admin(public_key_hex: String) -> Self {
        Self {
            role: DynamicClientRole::Admin,
            public_key_hex,
        }
    }
}

#[derive(Debug, Default)]
struct SessionFeatureState {
    logs_stream_requested: AtomicBool,
}

impl SessionFeatureState {
    fn record_hello_request(&self, request: &super::Envelope) {
        let Ok(hello) = payload_as::<HelloNegotiateRequest>(request) else {
            return;
        };
        if hello.validate().is_err() {
            return;
        }
        let requested = hello
            .required_features
            .iter()
            .any(|feature| feature == LOGS_STREAM_FEATURE);
        self.logs_stream_requested
            .store(requested, Ordering::Relaxed);
    }

    fn logs_stream_requested(&self) -> bool {
        self.logs_stream_requested.load(Ordering::Relaxed)
    }
}

#[async_trait]
pub(crate) trait ProtocolSession: Send + Sync {
    async fn accept_bi(&self) -> Option<(SendStream, RecvStream)>;

    fn max_datagram_size(&self) -> Option<usize> {
        None
    }

    fn send_datagram(&self, _payload: Bytes) -> Result<(), ImagodError> {
        Err(ImagodError::new(
            ErrorCode::Internal,
            "logs.datagram",
            "transport does not support datagrams",
        ))
    }

    fn peer_identity(&self) -> Option<Box<dyn Any>>;

    async fn closed(&self);
}

#[async_trait]
impl ProtocolSession for Session {
    async fn accept_bi(&self) -> Option<(SendStream, RecvStream)> {
        Session::accept_bi(self).await.ok()
    }

    fn max_datagram_size(&self) -> Option<usize> {
        Some(Session::max_datagram_size(self))
    }

    fn send_datagram(&self, payload: Bytes) -> Result<(), ImagodError> {
        Session::send_datagram(self, payload).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                "logs.datagram",
                format!("failed to send datagram: {e}"),
            )
        })
    }

    fn peer_identity(&self) -> Option<Box<dyn Any>> {
        quinn::Connection::peer_identity(self)
    }

    async fn closed(&self) {
        let _ = Session::closed(self).await;
    }
}

pub(crate) async fn run_session_loop<S>(
    handler: &ProtocolHandler,
    session: Arc<S>,
) -> Result<(), ImagodError>
where
    S: ProtocolSession + 'static,
{
    let auth_context = resolve_session_auth_context(session.as_ref())?;
    let session_features = Arc::new(SessionFeatureState::default());
    let mut stream_tasks = tokio::task::JoinSet::new();
    let mut first_error = None;
    loop {
        tokio::select! {
            accepted = session.accept_bi() => {
                let Some((mut send, mut recv)) = accepted else {
                    break;
                };
                let handler = handler.clone();
                let session = session.clone();
                let datagram_session: Arc<dyn ProtocolSession> = session.clone();
                let session_features = session_features.clone();
                let auth_context = auth_context.clone();
                stream_tasks.spawn(async move {
                    let close_signal = async move {
                        session.closed().await;
                    };
                    run_stream_io(
                        &handler,
                        datagram_session,
                        session_features,
                        auth_context,
                        &mut send,
                        &mut recv,
                        close_signal,
                    )
                    .await
                });
            }
            joined = stream_tasks.join_next(), if !stream_tasks.is_empty() => {
                collect_stream_task_result(joined, &mut first_error);
            }
        }
        if first_error.is_some() {
            break;
        }
    }

    if first_error.is_some() {
        stream_tasks.abort_all();
    }
    while let Some(joined) = stream_tasks.join_next().await {
        collect_stream_task_result(Some(joined), &mut first_error);
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) async fn run_local_stream(
    handler: &ProtocolHandler,
    stream: UnixStream,
) -> Result<(), ImagodError> {
    let auth_context = resolve_local_auth_context(&stream)?;
    let (mut recv, mut send) = stream.into_split();
    let Some(parsed) = (match read_single_local_request_envelope(&mut recv, handler).await {
        Ok(parsed) => parsed,
        Err(err) => {
            let envelope = error_envelope(
                MessageType::CommandEvent,
                Uuid::new_v4(),
                Uuid::new_v4(),
                err.to_structured(),
            );
            write_envelope(&mut send, &envelope, handler.frame_codec.as_ref()).await?;
            finish_stream(&mut send).await?;
            return Ok(());
        }
    }) else {
        finish_stream(&mut send).await?;
        return Ok(());
    };

    let close_signal = wait_for_local_disconnect(&mut recv);
    handle_parsed_request(
        handler,
        None,
        None,
        auth_context,
        &mut send,
        parsed,
        close_signal,
    )
    .await?;
    finish_stream(&mut send).await?;
    Ok(())
}

async fn run_stream_io<R, W, C>(
    handler: &ProtocolHandler,
    logs_session: Arc<dyn ProtocolSession>,
    session_features: Arc<SessionFeatureState>,
    auth_context: SessionAuthContext,
    send: &mut W,
    recv: &mut R,
    close_signal: C,
) -> Result<(), ImagodError>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
    C: Future<Output = ()> + Send,
{
    let Some(parsed) = (match read_single_request_envelope(recv, handler).await {
        Ok(parsed) => parsed,
        Err(err) => {
            let envelope = error_envelope(
                MessageType::CommandEvent,
                Uuid::new_v4(),
                Uuid::new_v4(),
                err.to_structured(),
            );
            write_envelope(send, &envelope, handler.frame_codec.as_ref()).await?;
            finish_stream(send).await?;
            return Ok(());
        }
    }) else {
        finish_stream(send).await?;
        return Ok(());
    };

    handle_parsed_request(
        handler,
        Some(logs_session),
        Some(session_features),
        auth_context,
        send,
        parsed,
        close_signal,
    )
    .await?;
    finish_stream(send).await?;
    Ok(())
}

async fn read_single_request_envelope<R>(
    recv: &mut R,
    handler: &ProtocolHandler,
) -> Result<Option<super::envelope_io::ParsedSingleRequestEnvelope>, ImagodError>
where
    R: AsyncRead + Unpin + Send,
{
    let buf = read_stream_with_timeout(recv, Duration::from_secs(STREAM_READ_TIMEOUT_SECS)).await?;
    parse_single_request_envelope(&buf, handler.frame_codec.as_ref())
}

#[cfg(unix)]
async fn read_single_local_request_envelope<R>(
    recv: &mut R,
    handler: &ProtocolHandler,
) -> Result<Option<super::envelope_io::ParsedSingleRequestEnvelope>, ImagodError>
where
    R: AsyncRead + Unpin + Send,
{
    let buf =
        read_terminated_stream_with_timeout(recv, Duration::from_secs(STREAM_READ_TIMEOUT_SECS))
            .await?;
    match buf {
        Some(buf) => parse_single_request_envelope(&buf, handler.frame_codec.as_ref()),
        None => Ok(None),
    }
}

async fn handle_parsed_request<W, C>(
    handler: &ProtocolHandler,
    logs_session: Option<Arc<dyn ProtocolSession>>,
    session_features: Option<Arc<SessionFeatureState>>,
    auth_context: SessionAuthContext,
    send: &mut W,
    parsed: super::envelope_io::ParsedSingleRequestEnvelope,
    close_signal: C,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
    C: Future<Output = ()> + Send,
{
    let request = parsed.request;
    let typed_push = parsed.typed_push;
    let request_id = request.request_id;
    let correlation_id = request.correlation_id;
    let request_message_type = request.message_type;
    if request_message_type == MessageType::HelloNegotiate
        && let Some(features) = &session_features
    {
        features.record_hello_request(&request);
    }
    if let Err(err) = ensure_message_type_allowed(&request, &auth_context) {
        let response = error_envelope(
            response_message_type_for_request(request_message_type),
            request_id,
            correlation_id,
            err.to_structured(),
        );
        write_envelope(send, &response, handler.frame_codec.as_ref()).await?;
        return Ok(());
    }

    if request_message_type == MessageType::ArtifactPush {
        let Some(payload) = typed_push else {
            let response = error_envelope(
                MessageType::ArtifactPush,
                request_id,
                correlation_id,
                ImagodError::new(
                    ErrorCode::BadRequest,
                    "protocol",
                    "request payload decode failed: missing artifact.push payload",
                )
                .to_structured(),
            );
            write_envelope(send, &response, handler.frame_codec.as_ref()).await?;
            return Ok(());
        };
        let response = match handler
            .handle_push_typed(request_id, correlation_id, payload)
            .await
        {
            Ok(resp) => resp,
            Err(err) => error_envelope(
                MessageType::ArtifactPush,
                request_id,
                correlation_id,
                err.to_structured(),
            ),
        };
        write_envelope(send, &response, handler.frame_codec.as_ref()).await?;
        return Ok(());
    }

    if request_message_type == MessageType::CommandStart {
        if let Err(err) = handler.handle_command_start(request, send).await {
            if !should_wrap_command_start_error(&err) {
                return Err(err);
            }
            let response = command_start_error_envelope(request_id, correlation_id, err);
            write_envelope(send, &response, handler.frame_codec.as_ref()).await?;
        }
        return Ok(());
    }
    if request_message_type == MessageType::LogsRequest {
        let client_requested_logs_stream = session_features
            .as_ref()
            .is_some_and(|features| features.logs_stream_requested());
        if let Err(err) = handler
            .handle_logs_request(
                logs_session,
                client_requested_logs_stream,
                request,
                send,
                close_signal,
            )
            .await
        {
            finish_logs_request_error(
                send,
                request_id,
                correlation_id,
                err,
                handler.frame_codec.as_ref(),
            )
            .await?;
        }
        return Ok(());
    }

    let response = match handler.handle_single(request, &auth_context).await {
        Ok(resp) => resp,
        Err(err) => error_envelope(
            response_message_type_for_request(request_message_type),
            request_id,
            correlation_id,
            err.to_structured(),
        ),
    };
    write_envelope(send, &response, handler.frame_codec.as_ref()).await?;
    Ok(())
}

async fn finish_logs_request_error<W>(
    send: &mut W,
    request_id: Uuid,
    correlation_id: Uuid,
    err: ImagodError,
    frame_codec: &impl super::codec::FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin,
{
    if is_benign_logs_request_write_error(&err) {
        return Ok(());
    }

    let response = error_envelope(
        MessageType::LogsRequest,
        request_id,
        correlation_id,
        err.to_structured(),
    );
    match write_envelope(send, &response, frame_codec).await {
        Err(write_err) if is_benign_logs_request_write_error(&write_err) => Ok(()),
        other => other,
    }
}

fn is_benign_logs_request_write_error(err: &ImagodError) -> bool {
    err.stage == "session.write"
}

fn resolve_session_auth_context<S>(session: &S) -> Result<SessionAuthContext, ImagodError>
where
    S: ProtocolSession + ?Sized,
{
    let public_key = extract_peer_public_key(session)?;
    let role = resolve_client_role(&public_key);
    Ok(SessionAuthContext {
        role,
        public_key_hex: encode_hex(&public_key),
    })
}

#[cfg(unix)]
fn resolve_local_auth_context(stream: &UnixStream) -> Result<SessionAuthContext, ImagodError> {
    let peer_cred = stream.peer_cred().map_err(|err| {
        ImagodError::new(
            ErrorCode::Unauthorized,
            "session.auth",
            format!("failed to inspect local control socket peer credentials: {err}"),
        )
    })?;
    let peer_uid = peer_cred.uid();
    authorize_local_uid(peer_uid, current_effective_uid())?;
    Ok(SessionAuthContext::local_admin(format!(
        "local-control-socket:{peer_uid}"
    )))
}

#[cfg(unix)]
fn authorize_local_uid(peer_uid: libc::uid_t, daemon_euid: libc::uid_t) -> Result<(), ImagodError> {
    if peer_uid == 0 || peer_uid == daemon_euid {
        return Ok(());
    }

    Err(ImagodError::new(
        ErrorCode::Unauthorized,
        "session.auth",
        "local control socket peer uid is not authorized",
    )
    .with_detail("peer_uid", peer_uid.to_string())
    .with_detail("daemon_euid", daemon_euid.to_string()))
}

#[cfg(unix)]
fn current_effective_uid() -> libc::uid_t {
    unsafe { libc::geteuid() }
}

fn extract_peer_public_key<S>(session: &S) -> Result<[u8; 32], ImagodError>
where
    S: ProtocolSession + ?Sized,
{
    let peer_identity = session.peer_identity().ok_or_else(|| {
        ImagodError::new(
            ErrorCode::Unauthorized,
            "session.auth",
            "peer identity is missing",
        )
    })?;
    let certificates = peer_identity
        .downcast::<Vec<CertificateDer<'static>>>()
        .map_err(|_| {
            ImagodError::new(
                ErrorCode::Unauthorized,
                "session.auth",
                "peer identity type is not certificate chain",
            )
        })?;
    let first_certificate = certificates.first().ok_or_else(|| {
        ImagodError::new(
            ErrorCode::Unauthorized,
            "session.auth",
            "peer certificate chain is empty",
        )
    })?;
    extract_ed25519_public_key(first_certificate.as_ref())
}

fn extract_ed25519_public_key(spki_der: &[u8]) -> Result<[u8; 32], ImagodError> {
    if spki_der.len() != ED25519_SPKI_PREFIX.len() + 32 {
        return Err(ImagodError::new(
            ErrorCode::Unauthorized,
            "session.auth",
            "peer public key (SPKI) must contain an ed25519 raw public key",
        ));
    }
    if !spki_der.starts_with(&ED25519_SPKI_PREFIX) {
        return Err(ImagodError::new(
            ErrorCode::Unauthorized,
            "session.auth",
            "peer public key (SPKI) is not ed25519",
        ));
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(&spki_der[ED25519_SPKI_PREFIX.len()..]);
    Ok(out)
}

fn resolve_client_role(public_key: &[u8; 32]) -> DynamicClientRole {
    resolve_dynamic_client_role(public_key)
}

fn encode_hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        write!(&mut out, "{byte:02x}").expect("writing to string should not fail");
    }
    out
}

fn ensure_message_type_allowed(
    request: &super::Envelope,
    auth_context: &SessionAuthContext,
) -> Result<(), ImagodError> {
    match auth_context.role {
        DynamicClientRole::Admin => Ok(()),
        DynamicClientRole::Client => {
            if matches!(
                request.message_type,
                MessageType::HelloNegotiate | MessageType::RpcInvoke
            ) {
                return Ok(());
            }

            Err(ImagodError::new(
                ErrorCode::Unauthorized,
                "session.authorize",
                format!(
                    "message type {} is not allowed for client role",
                    message_type_name(request.message_type)
                ),
            )
            .with_detail("role", "client")
            .with_detail("message_type", message_type_name(request.message_type))
            .with_detail("client_public_key", auth_context.public_key_hex.clone()))
        }
        DynamicClientRole::Unknown => Err(ImagodError::new(
            ErrorCode::Unauthorized,
            "session.authorize",
            "client public key does not have an assigned role",
        )
        .with_detail("role", "unknown")
        .with_detail("message_type", message_type_name(request.message_type))
        .with_detail("client_public_key", auth_context.public_key_hex.clone())),
    }
}

fn message_type_name(message_type: MessageType) -> &'static str {
    match message_type {
        MessageType::HelloNegotiate => "hello.negotiate",
        MessageType::DeployPrepare => "deploy.prepare",
        MessageType::ArtifactPush => "artifact.push",
        MessageType::ArtifactCommit => "artifact.commit",
        MessageType::CommandStart => "command.start",
        MessageType::CommandEvent => "command.event",
        MessageType::StateRequest => "state.request",
        MessageType::StateResponse => "state.response",
        MessageType::ServicesList => "services.list",
        MessageType::CommandCancel => "command.cancel",
        MessageType::LogsRequest => "logs.request",
        MessageType::LogsChunk => "logs.chunk",
        MessageType::LogsEnd => "logs.end",
        MessageType::RpcInvoke => "rpc.invoke",
        MessageType::BindingsCertInspect => "bindings.cert.inspect",
        MessageType::BindingsCertUpload => "bindings.cert.upload",
    }
}

fn command_start_error_envelope(
    request_id: Uuid,
    correlation_id: Uuid,
    err: ImagodError,
) -> super::Envelope {
    error_envelope(
        MessageType::CommandStart,
        request_id,
        correlation_id,
        err.to_structured(),
    )
}

fn should_wrap_command_start_error(err: &ImagodError) -> bool {
    err.stage == "command.start"
}

fn collect_stream_task_result(
    joined: Option<Result<Result<(), ImagodError>, tokio::task::JoinError>>,
    first_error: &mut Option<ImagodError>,
) {
    if first_error.is_some() {
        return;
    }
    let Some(joined) = joined else {
        return;
    };
    match joined {
        Ok(Ok(())) => {}
        Ok(Err(err)) => *first_error = Some(err),
        Err(err) => {
            *first_error = Some(ImagodError::new(
                ErrorCode::Internal,
                "session.loop",
                format!("stream task join failed: {err}"),
            ))
        }
    }
}

pub(crate) async fn read_stream_with_timeout<R>(
    recv: &mut R,
    timeout_duration: Duration,
) -> Result<Vec<u8>, ImagodError>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let limit = (MAX_STREAM_BYTES as u64).saturating_add(1);
    match tokio::time::timeout(timeout_duration, recv.take(limit).read_to_end(&mut buf)).await {
        Ok(result) => result
            .map_err(|e| {
                ImagodError::new(
                    imago_protocol::ErrorCode::BadRequest,
                    "session.read",
                    format!("failed to read stream: {e}"),
                )
            })
            .and_then(|_| {
                if buf.len() > MAX_STREAM_BYTES {
                    return Err(ImagodError::new(
                        imago_protocol::ErrorCode::BadRequest,
                        "session.read",
                        format!("stream exceeds max size {MAX_STREAM_BYTES} bytes"),
                    ));
                }
                Ok(buf)
            }),
        Err(_) => Err(stream_read_timeout_error()),
    }
}

#[cfg(unix)]
async fn read_terminated_stream_with_timeout<R>(
    recv: &mut R,
    timeout_duration: Duration,
) -> Result<Option<Vec<u8>>, ImagodError>
where
    R: AsyncRead + Unpin,
{
    match tokio::time::timeout(timeout_duration, read_terminated_stream(recv)).await {
        Ok(result) => result,
        Err(_) => Err(stream_read_timeout_error()),
    }
}

#[cfg(unix)]
async fn read_terminated_stream<R>(recv: &mut R) -> Result<Option<Vec<u8>>, ImagodError>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    loop {
        let mut header = [0u8; 4];
        match recv.read_exact(&mut header).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof && buf.is_empty() => {
                return Ok(None);
            }
            Err(err) => {
                return Err(ImagodError::new(
                    ErrorCode::BadRequest,
                    "session.read",
                    format!("failed to read stream: {err}"),
                ));
            }
        }

        let frame_len = u32::from_be_bytes(header) as usize;
        if frame_len == 0 {
            if buf.is_empty() {
                return Err(ImagodError::new(
                    ErrorCode::BadRequest,
                    "session.protocol",
                    "local control socket request must include one framed envelope",
                ));
            }
            return Ok(Some(buf));
        }

        let projected_len = buf
            .len()
            .checked_add(header.len())
            .and_then(|len| len.checked_add(frame_len))
            .ok_or_else(|| {
                ImagodError::new(
                    ErrorCode::BadRequest,
                    "session.read",
                    format!("stream exceeds max size {MAX_STREAM_BYTES} bytes"),
                )
            })?;
        if projected_len > MAX_STREAM_BYTES {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                "session.read",
                format!("stream exceeds max size {MAX_STREAM_BYTES} bytes"),
            ));
        }

        let mut payload = vec![0u8; frame_len];
        recv.read_exact(&mut payload).await.map_err(|err| {
            ImagodError::new(
                ErrorCode::BadRequest,
                "session.read",
                format!("failed to read stream: {err}"),
            )
        })?;
        buf.extend_from_slice(&header);
        buf.extend_from_slice(&payload);
    }
}

#[cfg(unix)]
async fn wait_for_local_disconnect<R>(recv: &mut R)
where
    R: AsyncRead + Unpin + Send,
{
    let mut buf = [0u8; 256];
    loop {
        match recv.read(&mut buf).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

pub(crate) fn stream_read_timeout_error() -> ImagodError {
    ImagodError::new(
        imago_protocol::ErrorCode::OperationTimeout,
        "session.read",
        format!(
            "stream read timed out after {} seconds",
            STREAM_READ_TIMEOUT_SECS
        ),
    )
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use std::{
        any::Any,
        pin::Pin,
        task::{Context, Poll},
        time::Duration,
    };

    use super::*;
    use crate::protocol_handler::{
        Envelope, lock_dynamic_public_keys_for_tests, replace_dynamic_public_keys_for_tests,
        upsert_dynamic_client_public_key,
    };
    use async_trait::async_trait;
    use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
    #[cfg(unix)]
    use tokio::net::UnixStream;

    fn hex_32(byte: u8) -> String {
        let mut out = String::with_capacity(64);
        for _ in 0..32 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    fn ed25519_spki_from_raw(raw: [u8; 32]) -> Vec<u8> {
        let mut spki = ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(&raw);
        spki
    }

    enum FakePeerIdentity {
        Missing,
        WrongType,
        EmptyChain,
        Ed25519([u8; 32]),
    }

    struct FakeProtocolSession {
        identity: FakePeerIdentity,
    }

    #[async_trait]
    impl ProtocolSession for FakeProtocolSession {
        async fn accept_bi(&self) -> Option<(SendStream, RecvStream)> {
            None
        }

        fn peer_identity(&self) -> Option<Box<dyn Any>> {
            match self.identity {
                FakePeerIdentity::Missing => None,
                FakePeerIdentity::WrongType => Some(Box::new("not-a-cert-chain".to_string())),
                FakePeerIdentity::EmptyChain => {
                    Some(Box::new(Vec::<CertificateDer<'static>>::new()))
                }
                FakePeerIdentity::Ed25519(raw) => Some(Box::new(vec![CertificateDer::from(
                    ed25519_spki_from_raw(raw),
                )])),
            }
        }

        async fn closed(&self) {}
    }

    struct FailingRead;

    impl AsyncRead for FailingRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::Error::other("boom")))
        }
    }

    #[test]
    fn resolve_client_role_observes_dynamic_updates() {
        let _guard = lock_dynamic_public_keys_for_tests();
        replace_dynamic_public_keys_for_tests(&[]);
        let key = [0x33u8; 32];

        assert_eq!(resolve_client_role(&key), DynamicClientRole::Unknown);

        upsert_dynamic_client_public_key(&hex_32(0x33))
            .expect("upsert should register client key in memory");
        assert_eq!(resolve_client_role(&key), DynamicClientRole::Client);
    }

    #[test]
    fn client_role_allows_only_hello_and_rpc_invokes() {
        let request_hello = Envelope::new(
            MessageType::HelloNegotiate,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({}),
        );
        let request_rpc = Envelope::new(
            MessageType::RpcInvoke,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({}),
        );
        let request_event = Envelope::new(
            MessageType::CommandEvent,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({}),
        );

        let client_context = SessionAuthContext {
            role: DynamicClientRole::Client,
            public_key_hex: hex_32(0x11),
        };

        assert!(ensure_message_type_allowed(&request_hello, &client_context).is_ok());
        assert!(ensure_message_type_allowed(&request_rpc, &client_context).is_ok());
        assert!(ensure_message_type_allowed(&request_event, &client_context).is_err());
    }

    #[test]
    fn given_hello_requests__when_record_session_features__then_logs_stream_flag_tracks_required_features()
     {
        let features = SessionFeatureState::default();
        let stream_hello = Envelope::new(
            MessageType::HelloNegotiate,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({
                "client_version": "0.1.0",
                "required_features": ["logs.request", "logs.stream"],
            }),
        );

        features.record_hello_request(&stream_hello);
        assert!(features.logs_stream_requested());

        let legacy_hello = Envelope::new(
            MessageType::HelloNegotiate,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({
                "client_version": "0.1.0",
                "required_features": ["logs.request"],
            }),
        );

        features.record_hello_request(&legacy_hello);
        assert!(!features.logs_stream_requested());
    }

    #[test]
    fn client_role_rejects_command_start() {
        let request = Envelope::new(
            MessageType::CommandStart,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({}),
        );
        let context = SessionAuthContext {
            role: DynamicClientRole::Client,
            public_key_hex: hex_32(0x11),
        };

        let err = ensure_message_type_allowed(&request, &context)
            .expect_err("client must not allow command.start");
        assert_eq!(err.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn unknown_role_rejects_any_message_type() {
        let request = Envelope::new(
            MessageType::HelloNegotiate,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({}),
        );
        let context = SessionAuthContext {
            role: DynamicClientRole::Unknown,
            public_key_hex: hex_32(0x22),
        };

        let err = ensure_message_type_allowed(&request, &context)
            .expect_err("unknown role must not allow message type");
        assert_eq!(err.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn message_type_name_supports_services_list() {
        assert_eq!(
            message_type_name(MessageType::ServicesList),
            "services.list"
        );
    }

    #[test]
    fn command_start_error_envelope_uses_command_start_message_type_and_request_ids() {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let envelope = command_start_error_envelope(
            request_id,
            correlation_id,
            ImagodError::new(
                ErrorCode::BadRequest,
                "command.start",
                "payload does not match command_type",
            ),
        );

        assert_eq!(envelope.message_type, MessageType::CommandStart);
        assert_eq!(envelope.request_id, request_id);
        assert_eq!(envelope.correlation_id, correlation_id);
        let error = envelope
            .error
            .expect("error envelope should include structured error");
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert_eq!(error.stage, "command.start");
        assert_eq!(error.message, "payload does not match command_type");
    }

    #[test]
    fn wraps_command_start_errors_before_acceptance_only() {
        let pre_accept = ImagodError::new(
            ErrorCode::BadRequest,
            "command.start",
            "payload does not match command_type",
        );
        assert!(should_wrap_command_start_error(&pre_accept));

        let post_accept = ImagodError::new(
            ErrorCode::Internal,
            "runtime.start",
            "wasi cli run trap: failed to create capture session",
        );
        assert!(!should_wrap_command_start_error(&post_accept));
    }

    #[test]
    fn given_spki_encoding__when_extract_ed25519_public_key__then_valid_and_invalid_cases_match_contract()
     {
        let raw = [0x5au8; 32];
        let spki = ed25519_spki_from_raw(raw);
        let parsed =
            extract_ed25519_public_key(&spki).expect("valid ed25519 spki should be accepted");
        assert_eq!(parsed, raw);

        let short = vec![0u8; ED25519_SPKI_PREFIX.len() + 31];
        let err = extract_ed25519_public_key(&short).expect_err("short spki should fail");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.auth");
        assert!(
            err.message
                .contains("must contain an ed25519 raw public key")
        );

        let mut wrong_prefix = ed25519_spki_from_raw(raw);
        wrong_prefix[8] = 0x71;
        let err = extract_ed25519_public_key(&wrong_prefix).expect_err("wrong algorithm must fail");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.auth");
        assert!(err.message.contains("is not ed25519"));
    }

    #[test]
    fn given_peer_identity_variants__when_extract_peer_public_key__then_errors_and_success_are_explicit()
     {
        let missing = FakeProtocolSession {
            identity: FakePeerIdentity::Missing,
        };
        let err = extract_peer_public_key(&missing).expect_err("missing identity should fail");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.auth");
        assert_eq!(err.message, "peer identity is missing");

        let wrong_type = FakeProtocolSession {
            identity: FakePeerIdentity::WrongType,
        };
        let err = extract_peer_public_key(&wrong_type).expect_err("wrong type should fail");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.auth");
        assert_eq!(err.message, "peer identity type is not certificate chain");

        let empty_chain = FakeProtocolSession {
            identity: FakePeerIdentity::EmptyChain,
        };
        let err = extract_peer_public_key(&empty_chain).expect_err("empty chain should fail");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.auth");
        assert_eq!(err.message, "peer certificate chain is empty");

        let valid = FakeProtocolSession {
            identity: FakePeerIdentity::Ed25519([0x7bu8; 32]),
        };
        let key = extract_peer_public_key(&valid).expect("valid chain should succeed");
        assert_eq!(key, [0x7bu8; 32]);
    }

    #[test]
    fn given_known_and_unknown_keys__when_resolve_session_auth_context__then_role_and_hex_are_set()
    {
        let _guard = lock_dynamic_public_keys_for_tests();
        replace_dynamic_public_keys_for_tests(&[]);
        upsert_dynamic_client_public_key(&hex_32(0x44)).expect("dynamic key upsert should succeed");

        let client = FakeProtocolSession {
            identity: FakePeerIdentity::Ed25519([0x44u8; 32]),
        };
        let context = resolve_session_auth_context(&client).expect("auth context should resolve");
        assert_eq!(context.role, DynamicClientRole::Client);
        assert_eq!(context.public_key_hex, hex_32(0x44));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn given_local_unix_peer__when_resolve_local_auth_context__then_same_euid_is_allowed() {
        let (server, _client) = UnixStream::pair().expect("unix stream pair should succeed");
        let context =
            resolve_local_auth_context(&server).expect("same-euid local peer should be allowed");

        assert_eq!(context.role, DynamicClientRole::Admin);
        assert!(context.public_key_hex.starts_with("local-control-socket:"));
    }

    #[cfg(unix)]
    #[test]
    fn given_mismatched_uid__when_authorize_local_uid__then_peer_is_rejected() {
        let err = authorize_local_uid(42, 7).expect_err("mismatched uid should be rejected");

        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.auth");
        assert_eq!(err.details.get("peer_uid").map(String::as_str), Some("42"));
        assert_eq!(
            err.details.get("daemon_euid").map(String::as_str),
            Some("7")
        );
    }

    #[test]
    fn given_client_role_denial__when_ensure_message_type_allowed__then_details_are_populated() {
        let request = Envelope::new(
            MessageType::CommandCancel,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({}),
        );
        let context = SessionAuthContext {
            role: DynamicClientRole::Client,
            public_key_hex: hex_32(0x11),
        };

        let err =
            ensure_message_type_allowed(&request, &context).expect_err("client should be denied");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.authorize");
        assert_eq!(err.details.get("role").map(String::as_str), Some("client"));
        assert_eq!(
            err.details.get("message_type").map(String::as_str),
            Some("command.cancel")
        );
        assert_eq!(
            err.details.get("client_public_key").map(String::as_str),
            Some(hex_32(0x11).as_str())
        );
    }

    #[test]
    fn given_unknown_role_denial__when_ensure_message_type_allowed__then_unknown_role_details_are_set()
     {
        let request = Envelope::new(
            MessageType::HelloNegotiate,
            Uuid::new_v4(),
            Uuid::new_v4(),
            serde_json::json!({}),
        );
        let context = SessionAuthContext {
            role: DynamicClientRole::Unknown,
            public_key_hex: hex_32(0x22),
        };

        let err = ensure_message_type_allowed(&request, &context)
            .expect_err("unknown role should be denied");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert_eq!(err.stage, "session.authorize");
        assert_eq!(err.details.get("role").map(String::as_str), Some("unknown"));
        assert_eq!(
            err.details.get("message_type").map(String::as_str),
            Some("hello.negotiate")
        );
    }

    #[test]
    fn given_message_types__when_message_type_name__then_protocol_names_are_stable() {
        let cases = [
            (MessageType::HelloNegotiate, "hello.negotiate"),
            (MessageType::DeployPrepare, "deploy.prepare"),
            (MessageType::ArtifactPush, "artifact.push"),
            (MessageType::ArtifactCommit, "artifact.commit"),
            (MessageType::CommandStart, "command.start"),
            (MessageType::CommandEvent, "command.event"),
            (MessageType::StateRequest, "state.request"),
            (MessageType::StateResponse, "state.response"),
            (MessageType::ServicesList, "services.list"),
            (MessageType::CommandCancel, "command.cancel"),
            (MessageType::LogsRequest, "logs.request"),
            (MessageType::LogsChunk, "logs.chunk"),
            (MessageType::LogsEnd, "logs.end"),
            (MessageType::RpcInvoke, "rpc.invoke"),
            (MessageType::BindingsCertUpload, "bindings.cert.upload"),
        ];

        for (message_type, expected) in cases {
            assert_eq!(message_type_name(message_type), expected);
        }
    }

    #[test]
    fn given_join_results__when_collect_stream_task_result__then_first_error_is_preserved() {
        let mut first_error = None;
        collect_stream_task_result(None, &mut first_error);
        assert!(first_error.is_none(), "none should not change first_error");

        collect_stream_task_result(Some(Ok(Ok(()))), &mut first_error);
        assert!(first_error.is_none(), "ok result should not set error");

        let stream_err = ImagodError::new(ErrorCode::Internal, "session.stream", "failed");
        collect_stream_task_result(Some(Ok(Err(stream_err))), &mut first_error);
        let err = first_error.expect("stream error should be captured");
        assert_eq!(err.stage, "session.stream");
    }

    #[tokio::test]
    async fn given_logs_request_write_error__when_finish_logs_request_error__then_it_is_ignored() {
        let mut send = tokio::io::sink();
        let err = ImagodError::new(ErrorCode::Internal, "session.write", "stream closed");

        finish_logs_request_error(
            &mut send,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            err,
            &crate::protocol_handler::codec::LengthPrefixedFrameCodec,
        )
        .await
        .expect("logs.request write failures should be ignored");
    }

    #[tokio::test]
    async fn given_logs_request_error_response_write_failure__when_finish_logs_request_error__then_it_is_ignored()
     {
        struct AlwaysFailWrite;

        impl tokio::io::AsyncWrite for AlwaysFailWrite {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                _buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Err(std::io::Error::other("forced write failure")))
            }

            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }

            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }

        let mut send = AlwaysFailWrite;
        let err = ImagodError::new(ErrorCode::Internal, "logs.request", "open failed");

        finish_logs_request_error(
            &mut send,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            err,
            &crate::protocol_handler::codec::LengthPrefixedFrameCodec,
        )
        .await
        .expect("logs.request error response write failures should be ignored");
    }

    #[tokio::test]
    async fn given_read_future_errors_or_times_out__when_read_stream_with_timeout__then_errors_are_mapped()
     {
        let mut failing = FailingRead;
        let err = read_stream_with_timeout(&mut failing, Duration::from_secs(1))
            .await
            .expect_err("read failure should be mapped");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, "session.read");
        assert!(err.message.contains("failed to read stream"));

        let (_writer, mut reader) = tokio::io::duplex(64);
        let timeout_err = read_stream_with_timeout(&mut reader, Duration::from_millis(1))
            .await
            .expect_err("pending future should timeout");
        assert_eq!(timeout_err.code, ErrorCode::OperationTimeout);
        assert_eq!(timeout_err.stage, "session.read");
        assert!(timeout_err.message.contains("timed out"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn given_local_request_without_disconnect__when_waiting_for_close__then_future_stays_pending_until_peer_drop()
     {
        let (mut client, server) = UnixStream::pair().expect("unix stream pair should succeed");
        let (mut recv, _send) = server.into_split();

        client
            .write_all(&[0, 0, 0, 5, b'h', b'e', b'l', b'l', b'o'])
            .await
            .expect("client should write request frame");
        client
            .write_all(&0u32.to_be_bytes())
            .await
            .expect("client should write request terminator");

        let request = read_terminated_stream_with_timeout(&mut recv, Duration::from_secs(1))
            .await
            .expect("request should read successfully")
            .expect("request should be present");
        assert_eq!(request, vec![0, 0, 0, 5, b'h', b'e', b'l', b'l', b'o']);

        let close = wait_for_local_disconnect(&mut recv);
        tokio::pin!(close);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut close)
                .await
                .is_err(),
            "close future should remain pending while peer stays connected"
        );

        drop(client);
        tokio::time::timeout(Duration::from_secs(1), &mut close)
            .await
            .expect("close future should resolve after peer drop");
    }

    #[test]
    fn given_raw_key_bytes__when_encode_hex__then_lower_hex_is_emitted() {
        let encoded = encode_hex(&[
            0x00, 0x11, 0xAA, 0xFF, 0x01, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90,
            0xA0, 0xB0, 0xC0, 0xD0, 0xE0, 0xF0, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF1,
            0x23, 0x45, 0x67, 0x89,
        ]);
        assert_eq!(
            encoded,
            "0011aaff01102030405060708090a0b0c0d0e0f0123456789abcdef123456789"
        );
    }
}
