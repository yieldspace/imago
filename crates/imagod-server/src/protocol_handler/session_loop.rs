use std::{any::Any, fmt::Write, future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::Bytes;
use imagod_common::ImagodError;
use imagod_spec::{ErrorCode, MessageType};
use rustls::pki_types::CertificateDer;
use uuid::Uuid;
use web_transport_quinn::{RecvStream, SendStream, Session};

use super::{
    DynamicClientRole, MAX_STREAM_BYTES, ProtocolHandler, STREAM_READ_TIMEOUT_SECS,
    envelope_io::{
        error_envelope, finish_stream, parse_single_request_envelope,
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

#[cfg(test)]
mod conformance_tests {
    use std::{any::Any, time::Duration};

    use async_trait::async_trait;
    use bytes::Bytes;
    use imagod_spec_formal::{
        SessionAuthProjectionAction, SessionAuthProjectionObservedState, SessionAuthProjectionSpec,
        SystemEffect, SystemState,
    };
    use nirvash_core::{
        ProtocolConformanceSpec, TransitionSystem,
        conformance::{ActionApplier, ProtocolRuntimeBinding, StateObserver},
    };
    use nirvash_macros::code_tests;
    use rustls::pki_types::CertificateDer;

    use super::*;
    use crate::protocol_handler::{
        Envelope, lock_dynamic_public_keys_for_tests, replace_dynamic_public_keys_for_tests,
        upsert_dynamic_client_public_key,
    };

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
        Ed25519([u8; 32]),
    }

    struct FakeProtocolSession {
        identity: FakePeerIdentity,
    }

    #[async_trait]
    impl ProtocolSession for FakeProtocolSession {
        async fn accept_bi(
            &self,
        ) -> Option<(
            web_transport_quinn::SendStream,
            web_transport_quinn::RecvStream,
        )> {
            None
        }

        fn max_datagram_size(&self) -> usize {
            1200
        }

        fn send_datagram(&self, _payload: Bytes) -> Result<(), ImagodError> {
            Ok(())
        }

        fn peer_identity(&self) -> Option<Box<dyn Any>> {
            match self.identity {
                FakePeerIdentity::Ed25519(raw) => Some(Box::new(vec![CertificateDer::from(
                    ed25519_spki_from_raw(raw),
                )])),
            }
        }

        async fn closed(&self) {}
    }

    #[derive(Debug)]
    struct SessionAuthRuntime {
        state: tokio::sync::Mutex<SystemState>,
        trace: tokio::sync::Mutex<Vec<SessionAuthProjectionAction>>,
    }

    impl SessionAuthRuntime {
        fn new() -> Self {
            Self {
                state: tokio::sync::Mutex::new(SessionAuthProjectionSpec::new().initial_state()),
                trace: tokio::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[derive(Debug, Default, Clone, Copy)]
    struct SessionAuthBinding;

    impl ProtocolRuntimeBinding<SessionAuthProjectionSpec> for SessionAuthBinding {
        type Runtime = SessionAuthRuntime;
        type Context = ();

        async fn fresh_runtime(_spec: &SessionAuthProjectionSpec) -> Self::Runtime {
            let _guard = lock_dynamic_public_keys_for_tests();
            replace_dynamic_public_keys_for_tests(&[], &[]);
            drop(_guard);
            SessionAuthRuntime::new()
        }

        fn context(_spec: &SessionAuthProjectionSpec) -> Self::Context {}
    }

    impl ActionApplier for SessionAuthRuntime {
        type Action = SessionAuthProjectionAction;
        type Output = Vec<SystemEffect>;
        type Context = ();

        async fn execute_action(
            &self,
            _context: &Self::Context,
            action: &Self::Action,
        ) -> Self::Output {
            let _guard = lock_dynamic_public_keys_for_tests();
            let spec = SessionAuthProjectionSpec::new();
            let mut state = self.state.lock().await;
            let prev = state.clone();
            let Some(next) = spec.transition(&prev, action) else {
                return Vec::new();
            };

            match action {
                SessionAuthProjectionAction::AcceptSession => {}
                SessionAuthProjectionAction::AuthenticateAdmin => {
                    replace_dynamic_public_keys_for_tests(&[[0x11u8; 32]], &[]);
                    let session = FakeProtocolSession {
                        identity: FakePeerIdentity::Ed25519([0x11u8; 32]),
                    };
                    let context =
                        resolve_session_auth_context(&session).expect("admin auth should resolve");
                    assert_eq!(context.role, DynamicClientRole::Admin);
                }
                SessionAuthProjectionAction::AuthenticateClient => {
                    replace_dynamic_public_keys_for_tests(&[], &[[0x22u8; 32]]);
                    let session = FakeProtocolSession {
                        identity: FakePeerIdentity::Ed25519([0x22u8; 32]),
                    };
                    let context =
                        resolve_session_auth_context(&session).expect("client auth should resolve");
                    assert_eq!(context.role, DynamicClientRole::Client);
                }
                SessionAuthProjectionAction::AuthenticateUnknown => {
                    replace_dynamic_public_keys_for_tests(&[], &[]);
                    let session = FakeProtocolSession {
                        identity: FakePeerIdentity::Ed25519([0x33u8; 32]),
                    };
                    let context = resolve_session_auth_context(&session)
                        .expect("unknown auth context should resolve");
                    assert_eq!(context.role, DynamicClientRole::Unknown);
                }
                SessionAuthProjectionAction::AuthorizeAdminServicesList => {
                    let request = Envelope::new(
                        MessageType::ServicesList,
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        serde_json::json!({}),
                    );
                    ensure_message_type_allowed(
                        &request,
                        &SessionAuthContext {
                            role: DynamicClientRole::Admin,
                            public_key_hex: hex_32(0x11),
                        },
                    )
                    .expect("admin should authorize services.list");
                }
                SessionAuthProjectionAction::AuthorizeClientHello => {
                    let request = Envelope::new(
                        MessageType::HelloNegotiate,
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        serde_json::json!({}),
                    );
                    ensure_message_type_allowed(
                        &request,
                        &SessionAuthContext {
                            role: DynamicClientRole::Client,
                            public_key_hex: hex_32(0x22),
                        },
                    )
                    .expect("client should authorize hello.negotiate");
                }
                SessionAuthProjectionAction::AuthorizeClientRpc => {
                    let request = Envelope::new(
                        MessageType::RpcInvoke,
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        serde_json::json!({}),
                    );
                    ensure_message_type_allowed(
                        &request,
                        &SessionAuthContext {
                            role: DynamicClientRole::Client,
                            public_key_hex: hex_32(0x22),
                        },
                    )
                    .expect("client should authorize rpc.invoke");
                }
                SessionAuthProjectionAction::RejectUnauthorizedServicesList => {
                    let request = Envelope::new(
                        MessageType::ServicesList,
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        serde_json::json!({}),
                    );
                    let err = ensure_message_type_allowed(
                        &request,
                        &SessionAuthContext {
                            role: DynamicClientRole::Unknown,
                            public_key_hex: hex_32(0x33),
                        },
                    )
                    .expect_err("unknown role should be rejected");
                    assert_eq!(err.code, ErrorCode::Unauthorized);
                }
                SessionAuthProjectionAction::ReadTimeout => {
                    let err = read_stream_with_timeout(
                        std::future::pending::<Result<Vec<u8>, std::io::Error>>(),
                        Duration::from_millis(1),
                    )
                    .await
                    .expect_err("timeout should fail");
                    assert_eq!(err.code, ErrorCode::OperationTimeout);
                }
                SessionAuthProjectionAction::CloseStream => {}
                SessionAuthProjectionAction::UploadClientAuthority => {
                    upsert_dynamic_client_public_key(&hex_32(0x22))
                        .expect("authority upload should succeed");
                }
            }

            *state = next;
            self.trace.lock().await.push(*action);
            spec.expected_output(&prev, action, Some(&*state))
        }
    }

    impl StateObserver for SessionAuthRuntime {
        type ObservedState = SessionAuthProjectionObservedState;
        type Context = ();

        async fn observe_state(&self, _context: &Self::Context) -> Self::ObservedState {
            SessionAuthProjectionObservedState {
                trace: self.trace.lock().await.clone(),
            }
        }
    }

    #[code_tests(spec = SessionAuthProjectionSpec, binding = SessionAuthBinding)]
    const _: () = ();
}

#[async_trait]
pub(crate) trait ProtocolSession: Send + Sync {
    async fn accept_bi(&self) -> Option<(SendStream, RecvStream)>;

    fn max_datagram_size(&self) -> usize;

    fn send_datagram(&self, payload: Bytes) -> Result<(), ImagodError>;

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

    let parsed = match parse_single_request_envelope(&buf, handler.frame_codec.as_ref()) {
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
    let Some(parsed) = parsed else {
        finish_stream(&mut send)?;
        return Ok(());
    };
    let request = parsed.request;
    let typed_push = parsed.typed_push;
    let request_id = request.request_id;
    let correlation_id = request.correlation_id;
    let request_message_type = request.message_type;
    if let Err(err) = ensure_message_type_allowed(&request, &auth_context) {
        let response = error_envelope(
            response_message_type_for_request(request_message_type),
            request_id,
            correlation_id,
            err.to_structured(),
        );
        write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        finish_stream(&mut send)?;
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
            write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
            finish_stream(&mut send)?;
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
        write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        finish_stream(&mut send)?;
        return Ok(());
    }

    if request_message_type == MessageType::CommandStart {
        if let Err(err) = handler.handle_command_start(request, &mut send).await {
            if !should_wrap_command_start_error(&err) {
                return Err(err);
            }
            let response = command_start_error_envelope(request_id, correlation_id, err);
            write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        }
        finish_stream(&mut send)?;
        return Ok(());
    }
    if request_message_type == MessageType::LogsRequest {
        if let Err(err) = handler
            .handle_logs_request(session.clone(), request, &mut send)
            .await
        {
            let response = error_envelope(
                MessageType::LogsRequest,
                request_id,
                correlation_id,
                err.to_structured(),
            );
            write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        }
        finish_stream(&mut send)?;
        return Ok(());
    }

    let response = match handler.handle_single(request).await {
        Ok(resp) => resp,
        Err(err) => error_envelope(
            response_message_type_for_request(request_message_type),
            request_id,
            correlation_id,
            err.to_structured(),
        ),
    };
    write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
    finish_stream(&mut send)?;
    Ok(())
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
                ErrorCode::BadRequest,
                "session.read",
                format!("failed to read stream: {e}"),
            )
        }),
        Err(_) => Err(stream_read_timeout_error()),
    }
}

pub(crate) fn stream_read_timeout_error() -> ImagodError {
    ImagodError::new(
        ErrorCode::OperationTimeout,
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
    use std::{any::Any, time::Duration};

    use super::*;
    use crate::protocol_handler::{
        Envelope, lock_dynamic_public_keys_for_tests, replace_dynamic_public_keys_for_tests,
        upsert_dynamic_client_public_key,
    };
    use async_trait::async_trait;

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

        fn max_datagram_size(&self) -> usize {
            1200
        }

        fn send_datagram(&self, _payload: Bytes) -> Result<(), ImagodError> {
            Ok(())
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

    #[test]
    fn resolve_client_role_observes_dynamic_updates() {
        let _guard = lock_dynamic_public_keys_for_tests();
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
        replace_dynamic_public_keys_for_tests(&[], &[]);
        upsert_dynamic_client_public_key(&hex_32(0x44)).expect("dynamic key upsert should succeed");

        let client = FakeProtocolSession {
            identity: FakePeerIdentity::Ed25519([0x44u8; 32]),
        };
        let context = resolve_session_auth_context(&client).expect("auth context should resolve");
        assert_eq!(context.role, DynamicClientRole::Client);
        assert_eq!(context.public_key_hex, hex_32(0x44));
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
    async fn given_read_future_errors_or_times_out__when_read_stream_with_timeout__then_errors_are_mapped()
     {
        let err = read_stream_with_timeout(
            async { Err::<Vec<u8>, _>(std::io::Error::other("boom")) },
            Duration::from_secs(1),
        )
        .await
        .expect_err("read failure should be mapped");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, "session.read");
        assert!(err.message.contains("failed to read stream"));

        let timeout_err = read_stream_with_timeout(
            std::future::pending::<Result<Vec<u8>, std::io::Error>>(),
            Duration::from_millis(1),
        )
        .await
        .expect_err("pending future should timeout");
        assert_eq!(timeout_err.code, ErrorCode::OperationTimeout);
        assert_eq!(timeout_err.stage, "session.read");
        assert!(timeout_err.message.contains("timed out"));
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
