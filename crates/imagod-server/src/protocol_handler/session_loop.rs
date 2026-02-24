use std::{any::Any, fmt::Write, future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use imago_protocol::{ErrorCode, MessageType};
use imagod_common::ImagodError;
use rustls::pki_types::CertificateDer;
use uuid::Uuid;
use web_transport_quinn::{RecvStream, SendStream, Session};

use super::{
    DynamicClientRole, MAX_STREAM_BYTES, ProtocolHandler, STREAM_READ_TIMEOUT_SECS,
    envelope_io::{
        ensure_single_request_envelope, error_envelope, finish_stream, parse_stream_envelopes,
        response_message_type_for_request, write_envelope,
    },
    resolve_dynamic_client_role,
};

const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionAuthContext {
    role: DynamicClientRole,
    public_key_hex: String,
}

#[async_trait]
pub(crate) trait ProtocolSession: Send + Sync {
    async fn accept_bi(&self) -> Option<(SendStream, RecvStream)>;

    fn max_datagram_size(&self) -> usize;

    fn send_datagram(&self, payload: Vec<u8>) -> Result<(), ImagodError>;

    fn peer_identity(&self) -> Option<Box<dyn Any>>;

    async fn closed(&self);
}

#[async_trait]
impl ProtocolSession for Session {
    async fn accept_bi(&self) -> Option<(SendStream, RecvStream)> {
        Session::accept_bi(self).await.ok()
    }

    fn max_datagram_size(&self) -> usize {
        Session::max_datagram_size(self)
    }

    fn send_datagram(&self, payload: Vec<u8>) -> Result<(), ImagodError> {
        Session::send_datagram(self, payload.into()).map_err(|e| {
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
    let auth_context = resolve_session_auth_context(handler, session.as_ref())?;
    let mut stream_tasks = tokio::task::JoinSet::new();
    let mut first_error = None;
    loop {
        tokio::select! {
            accepted = session.accept_bi() => {
                let Some((send, recv)) = accepted else {
                    break;
                };
                let handler = handler.clone();
                let session = session.clone();
                let auth_context = auth_context.clone();
                stream_tasks.spawn(async move {
                    run_single_stream(&handler, session, auth_context, send, recv).await
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

async fn run_single_stream<S>(
    handler: &ProtocolHandler,
    session: Arc<S>,
    auth_context: SessionAuthContext,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<(), ImagodError>
where
    S: ProtocolSession + 'static,
{
    let buf = match read_stream_with_timeout(
        recv.read_to_end(MAX_STREAM_BYTES),
        Duration::from_secs(STREAM_READ_TIMEOUT_SECS),
    )
    .await
    {
        Ok(buf) => buf,
        Err(err) => {
            let envelope = error_envelope(
                MessageType::CommandEvent,
                Uuid::new_v4(),
                Uuid::new_v4(),
                err.to_structured(),
            );
            write_envelope(&mut send, &envelope, handler.frame_codec.as_ref()).await?;
            finish_stream(&mut send)?;
            return Ok(());
        }
    };

    let envelopes = match parse_stream_envelopes(&buf, handler.frame_codec.as_ref()) {
        Ok(v) => v,
        Err(err) => {
            let envelope = error_envelope(
                MessageType::CommandEvent,
                Uuid::new_v4(),
                Uuid::new_v4(),
                err.to_structured(),
            );
            write_envelope(&mut send, &envelope, handler.frame_codec.as_ref()).await?;
            finish_stream(&mut send)?;
            return Ok(());
        }
    };

    if envelopes.is_empty() {
        finish_stream(&mut send)?;
        return Ok(());
    }

    if let Err(err) = ensure_single_request_envelope(&envelopes) {
        let first = &envelopes[0];
        let response = error_envelope(
            response_message_type_for_request(first.message_type),
            first.request_id,
            first.correlation_id,
            err.to_structured(),
        );
        write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        finish_stream(&mut send)?;
        return Ok(());
    }

    let request = envelopes[0].clone();
    if let Err(err) = ensure_message_type_allowed(&request, &auth_context) {
        let response = error_envelope(
            response_message_type_for_request(request.message_type),
            request.request_id,
            request.correlation_id,
            err.to_structured(),
        );
        write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        finish_stream(&mut send)?;
        return Ok(());
    }

    if request.message_type == MessageType::CommandStart {
        let request_id = request.request_id;
        let correlation_id = request.correlation_id;
        if let Err(err) = handler.handle_command_start(request, &mut send).await {
            let response = command_start_error_envelope(request_id, correlation_id, err);
            write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        }
        finish_stream(&mut send)?;
        return Ok(());
    }
    if request.message_type == MessageType::LogsRequest {
        if let Err(err) = handler
            .handle_logs_request(session.clone(), request.clone(), &mut send)
            .await
        {
            let response = error_envelope(
                MessageType::LogsRequest,
                request.request_id,
                request.correlation_id,
                err.to_structured(),
            );
            write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        }
        finish_stream(&mut send)?;
        return Ok(());
    }

    let response = match handler.handle_single(request.clone()).await {
        Ok(resp) => resp,
        Err(err) => error_envelope(
            response_message_type_for_request(request.message_type),
            request.request_id,
            request.correlation_id,
            err.to_structured(),
        ),
    };
    write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
    finish_stream(&mut send)?;
    Ok(())
}

fn resolve_session_auth_context<S>(
    _handler: &ProtocolHandler,
    session: &S,
) -> Result<SessionAuthContext, ImagodError>
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

pub(crate) async fn read_stream_with_timeout<F, E>(
    read_future: F,
    timeout_duration: Duration,
) -> Result<Vec<u8>, ImagodError>
where
    F: Future<Output = Result<Vec<u8>, E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(timeout_duration, read_future).await {
        Ok(result) => result.map_err(|e| {
            ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "session.read",
                format!("failed to read stream: {e}"),
            )
        }),
        Err(_) => Err(stream_read_timeout_error()),
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
    use std::sync::Mutex;

    use super::*;
    use crate::protocol_handler::{
        Envelope, replace_dynamic_public_keys_for_tests, upsert_dynamic_client_public_key,
    };

    static DYNAMIC_KEYS_TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn hex_32(byte: u8) -> String {
        let mut out = String::with_capacity(64);
        for _ in 0..32 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    #[test]
    fn resolve_client_role_observes_dynamic_updates() {
        let _guard = DYNAMIC_KEYS_TEST_MUTEX
            .lock()
            .expect("mutex lock should succeed");
        replace_dynamic_public_keys_for_tests(&[], &[]);
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
}
