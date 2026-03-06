use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use imago_protocol::{
    LogChunk, LogEnd, LogError, LogErrorCode, LogStreamKind, MessageType, ProtocolEnvelope, to_cbor,
};
use imagod_common::ImagodError;
use imagod_control::{
    ServiceLogEvent, ServiceLogSnapshot, ServiceLogStream, ServiceLogSubscription,
};
use tokio::{sync::mpsc, time::Duration};
use uuid::Uuid;

use super::{LOG_DATAGRAM_TARGET_BYTES, session_loop::ProtocolSession};
const DATAGRAM_SEND_RETRY_DELAYS_MS: [u64; 3] = [10, 50, 100];

#[async_trait]
pub(crate) trait LogsForwarder: Send + Sync {
    async fn forward<S>(
        &self,
        session: Arc<S>,
        request_id: Uuid,
        correlation_id: Uuid,
        subscriptions: Vec<ServiceLogSubscription>,
        with_timestamp: bool,
    ) where
        S: ProtocolSession + 'static;
}

pub(crate) struct DefaultLogsForwarder;

#[async_trait]
impl LogsForwarder for DefaultLogsForwarder {
    async fn forward<S>(
        &self,
        session: Arc<S>,
        request_id: Uuid,
        correlation_id: Uuid,
        subscriptions: Vec<ServiceLogSubscription>,
        with_timestamp: bool,
    ) where
        S: ProtocolSession + 'static,
    {
        run_logs_forwarder(
            session,
            request_id,
            correlation_id,
            subscriptions,
            with_timestamp,
        )
        .await;
    }
}

pub(crate) async fn run_logs_forwarder<S>(
    session: Arc<S>,
    request_id: Uuid,
    correlation_id: Uuid,
    subscriptions: Vec<ServiceLogSubscription>,
    with_timestamp: bool,
) where
    S: ProtocolSession + 'static,
{
    if subscriptions.is_empty() {
        return;
    }

    let max_datagram_size = session.max_datagram_size();
    let fallback_name = subscriptions[0].service_name.clone();
    let service_names = subscriptions
        .iter()
        .map(|subscription| subscription.service_name.clone())
        .collect::<Vec<_>>();
    let mut seq = 0u64;
    let mut last_name: Option<String> = None;
    let chunk_size = match fixed_log_chunk_size(
        request_id,
        correlation_id,
        max_datagram_size,
        &service_names,
        with_timestamp,
    ) {
        Ok(size) => size,
        Err(err) => {
            let _ = send_logs_end_datagram(
                session.as_ref(),
                request_id,
                correlation_id,
                max_datagram_size,
                seq,
                Some(log_error_from_imagod_error(&err)),
            )
            .await;
            return;
        }
    };
    let sender = LogsDatagramSender::new(
        session.as_ref(),
        request_id,
        correlation_id,
        max_datagram_size,
        chunk_size,
        with_timestamp,
    );

    let stream_result = stream_logs_datagrams(
        session.as_ref(),
        &sender,
        subscriptions,
        &mut seq,
        &mut last_name,
    )
    .await;

    match stream_result {
        Ok(()) => {
            let terminal_name = last_name.unwrap_or(fallback_name);
            let _ = sender
                .send_single_log_chunk(
                    &mut seq,
                    &terminal_name,
                    LogStreamKind::Composite,
                    &[],
                    true,
                    None,
                )
                .await;
            let _ = sender.send_logs_end_datagram(seq, None).await;
        }
        Err(err) => {
            let _ = sender
                .send_logs_end_datagram(seq, Some(log_error_from_imagod_error(&err)))
                .await;
        }
    }
}

struct LogsDatagramSender<'a, S>
where
    S: ProtocolSession,
{
    session: &'a S,
    request_id: Uuid,
    correlation_id: Uuid,
    max_datagram_size: usize,
    chunk_size: usize,
    with_timestamp: bool,
}

#[derive(serde::Serialize)]
struct BorrowedLogChunk<'a> {
    request_id: Uuid,
    seq: u64,
    name: &'a str,
    stream_kind: LogStreamKind,
    #[serde(with = "serde_bytes")]
    bytes: &'a [u8],
    is_last: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timestamp_unix_ms: Option<u64>,
}

impl<'a, S> LogsDatagramSender<'a, S>
where
    S: ProtocolSession,
{
    fn new(
        session: &'a S,
        request_id: Uuid,
        correlation_id: Uuid,
        max_datagram_size: usize,
        chunk_size: usize,
        with_timestamp: bool,
    ) -> Self {
        Self {
            session,
            request_id,
            correlation_id,
            max_datagram_size,
            chunk_size,
            with_timestamp,
        }
    }

    async fn send_log_data_chunks(
        &self,
        seq: &mut u64,
        name: &str,
        stream_kind: LogStreamKind,
        bytes: &[u8],
        timestamp_unix_ms: Option<u64>,
        last_name: &mut Option<String>,
    ) -> Result<(), ImagodError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.chunk_size == 0 {
            return Err(ImagodError::new(
                imago_protocol::ErrorCode::Internal,
                "logs.datagram",
                "computed logs chunk size must be greater than zero",
            ));
        }

        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = bytes.len().min(offset.saturating_add(self.chunk_size));
            self.send_single_log_chunk(
                seq,
                name,
                stream_kind,
                &bytes[offset..end],
                false,
                timestamp_unix_ms,
            )
            .await?;
            offset = end;
        }
        *last_name = Some(name.to_string());

        Ok(())
    }

    async fn send_single_log_chunk(
        &self,
        seq: &mut u64,
        name: &str,
        stream_kind: LogStreamKind,
        bytes: &[u8],
        is_last: bool,
        timestamp_unix_ms: Option<u64>,
    ) -> Result<(), ImagodError> {
        let timestamp_unix_ms = if self.with_timestamp {
            timestamp_unix_ms
        } else {
            None
        };
        let chunk = BorrowedLogChunk {
            request_id: self.request_id,
            seq: *seq,
            name,
            stream_kind,
            bytes,
            is_last,
            timestamp_unix_ms,
        };
        let envelope = ProtocolEnvelope::new(
            MessageType::LogsChunk,
            self.request_id,
            self.correlation_id,
            chunk,
        );
        send_datagram_envelope(self.session, &envelope, self.max_datagram_size).await?;
        *seq = seq.saturating_add(1);
        Ok(())
    }

    async fn send_logs_end_datagram(
        &self,
        seq: u64,
        error: Option<LogError>,
    ) -> Result<(), ImagodError> {
        send_logs_end_datagram(
            self.session,
            self.request_id,
            self.correlation_id,
            self.max_datagram_size,
            seq,
            error,
        )
        .await
    }
}

async fn send_logs_end_datagram<S>(
    session: &S,
    request_id: Uuid,
    correlation_id: Uuid,
    max_datagram_size: usize,
    seq: u64,
    error: Option<LogError>,
) -> Result<(), ImagodError>
where
    S: ProtocolSession,
{
    let end = LogEnd {
        request_id,
        seq,
        error,
    };
    let envelope = ProtocolEnvelope::new(MessageType::LogsEnd, request_id, correlation_id, end);
    send_datagram_envelope(session, &envelope, max_datagram_size).await
}

async fn stream_logs_datagrams<S>(
    session: &S,
    sender: &LogsDatagramSender<'_, S>,
    subscriptions: Vec<ServiceLogSubscription>,
    seq: &mut u64,
    last_name: &mut Option<String>,
) -> Result<(), ImagodError>
where
    S: ProtocolSession,
{
    for subscription in &subscriptions {
        match &subscription.snapshot {
            ServiceLogSnapshot::Bytes(bytes) => {
                sender
                    .send_log_data_chunks(
                        seq,
                        &subscription.service_name,
                        LogStreamKind::Composite,
                        bytes,
                        None,
                        last_name,
                    )
                    .await?;
            }
            ServiceLogSnapshot::Events(events) => {
                if sender.with_timestamp {
                    for event in events {
                        sender
                            .send_log_data_chunks(
                                seq,
                                &subscription.service_name,
                                LogStreamKind::Composite,
                                &event.bytes,
                                Some(event.timestamp_unix_ms),
                                last_name,
                            )
                            .await?;
                    }
                } else {
                    let bytes = flatten_log_event_bytes(events);
                    sender
                        .send_log_data_chunks(
                            seq,
                            &subscription.service_name,
                            LogStreamKind::Composite,
                            &bytes,
                            None,
                            last_name,
                        )
                        .await?;
                }
            }
        }
    }

    let mut follow_targets = subscriptions
        .into_iter()
        .filter_map(|subscription| {
            subscription
                .receiver
                .map(|receiver| (subscription.service_name, receiver))
        })
        .collect::<Vec<_>>();
    if follow_targets.is_empty() {
        return Ok(());
    }

    enum FollowForwardMsg {
        Event {
            service_name: String,
            event: ServiceLogEvent,
        },
        Lagged {
            service_name: String,
            dropped: u64,
        },
    }

    let (tx, mut rx) = mpsc::channel::<FollowForwardMsg>(128);
    let mut forward_tasks = Vec::new();
    for (service_name, mut receiver) in follow_targets.drain(..) {
        let tx = tx.clone();
        let handle = tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if tx
                            .send(FollowForwardMsg::Event {
                                service_name: service_name.clone(),
                                event,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => {
                        if tx
                            .send(FollowForwardMsg::Lagged {
                                service_name: service_name.clone(),
                                dropped,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        forward_tasks.push(handle);
    }
    drop(tx);

    loop {
        tokio::select! {
            maybe_msg = rx.recv() => {
                let Some(message) = maybe_msg else {
                    break;
                };
                match message {
                    FollowForwardMsg::Event { service_name, event } => {
                        sender
                            .send_log_data_chunks(
                                seq,
                                &service_name,
                                service_log_stream_to_protocol(event.stream),
                                &event.bytes,
                                Some(event.timestamp_unix_ms),
                                last_name,
                            )
                            .await?;
                    }
                    FollowForwardMsg::Lagged { service_name, dropped } => {
                        *last_name = Some(service_name);
                        advance_seq_for_lagged(seq, dropped);
                    }
                }
            }
            _ = session.closed() => break,
        }
    }

    for task in forward_tasks {
        task.abort();
    }

    Ok(())
}

pub(crate) fn advance_seq_for_lagged(seq: &mut u64, dropped: u64) {
    *seq = seq.saturating_add(dropped);
}

fn flatten_log_event_bytes(events: &[ServiceLogEvent]) -> Vec<u8> {
    let total = events.iter().map(|event| event.bytes.len()).sum();
    let mut out = Vec::with_capacity(total);
    for event in events {
        out.extend_from_slice(&event.bytes);
    }
    out
}

async fn send_datagram_envelope<S, T>(
    session: &S,
    envelope: &ProtocolEnvelope<T>,
    max_datagram_size: usize,
) -> Result<(), ImagodError>
where
    S: ProtocolSession,
    T: serde::Serialize,
{
    let bytes = to_cbor(envelope).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!("failed to encode datagram payload: {e}"),
        )
    })?;
    if bytes.len() > max_datagram_size {
        return Err(ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!(
                "datagram payload too large: size={} max={}",
                bytes.len(),
                max_datagram_size
            ),
        ));
    }
    send_datagram_with_retry(session, Bytes::from(bytes)).await
}

pub(super) async fn send_datagram_with_retry<S>(
    session: &S,
    bytes: Bytes,
) -> Result<(), ImagodError>
where
    S: ProtocolSession,
{
    match session.send_datagram(bytes.clone()) {
        Ok(()) => Ok(()),
        Err(err) => {
            let mut last_err = err;
            for delay_ms in DATAGRAM_SEND_RETRY_DELAYS_MS {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                match session.send_datagram(bytes.clone()) {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        last_err = err;
                    }
                }
            }
            Err(last_err)
        }
    }
}

pub(crate) fn fixed_log_chunk_size(
    request_id: Uuid,
    correlation_id: Uuid,
    max_datagram_size: usize,
    service_names: &[String],
    with_timestamp: bool,
) -> Result<usize, ImagodError> {
    let name = service_names
        .iter()
        .max_by_key(|name| name.len())
        .cloned()
        .unwrap_or_else(|| "logs".to_string());
    let probe = LogChunk {
        request_id,
        seq: u64::MAX,
        name,
        stream_kind: LogStreamKind::Composite,
        bytes: Vec::new(),
        is_last: false,
        timestamp_unix_ms: with_timestamp.then_some(u64::MAX),
    };
    let envelope = ProtocolEnvelope::new(MessageType::LogsChunk, request_id, correlation_id, probe);
    let overhead = to_cbor(&envelope).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!("failed to encode datagram probe: {e}"),
        )
    })?;
    let computed_limit = max_datagram_size.saturating_sub(overhead.len().saturating_add(2));
    let chunk_size = computed_limit.min(LOG_DATAGRAM_TARGET_BYTES);
    if chunk_size == 0 {
        return Err(ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!(
                "datagram size is too small for logs payload: max={} overhead={}",
                max_datagram_size,
                overhead.len()
            ),
        ));
    }

    Ok(chunk_size)
}

pub(crate) fn service_log_stream_to_protocol(stream: ServiceLogStream) -> LogStreamKind {
    match stream {
        ServiceLogStream::Stdout => LogStreamKind::Stdout,
        ServiceLogStream::Stderr => LogStreamKind::Stderr,
    }
}

pub(crate) fn log_error_from_imagod_error(err: &ImagodError) -> LogError {
    let code = match err.code {
        imago_protocol::ErrorCode::NotFound => LogErrorCode::ProcessNotFound,
        imago_protocol::ErrorCode::Unauthorized => LogErrorCode::PermissionDenied,
        _ => LogErrorCode::Internal,
    };

    LogError {
        code,
        message: err.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    use std::{
        any::Any,
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use imago_protocol::{ErrorCode, from_cbor};
    use imagod_control::{ServiceLogEvent, ServiceLogStream, ServiceLogSubscription};
    use tokio::sync::{Notify, broadcast};

    use super::*;
    use crate::protocol_handler::session_loop::ProtocolSession;

    struct FakeProtocolSession {
        max_datagram_size: usize,
        send_outcomes: Mutex<VecDeque<Result<(), String>>>,
        sent_datagrams: Mutex<Vec<Vec<u8>>>,
        sent_payload_ptrs: Mutex<Vec<usize>>,
        send_attempts: AtomicUsize,
        close_notify: Notify,
    }

    impl FakeProtocolSession {
        fn new(max_datagram_size: usize, send_outcomes: Vec<Result<(), String>>) -> Self {
            Self {
                max_datagram_size,
                send_outcomes: Mutex::new(send_outcomes.into()),
                sent_datagrams: Mutex::new(Vec::new()),
                sent_payload_ptrs: Mutex::new(Vec::new()),
                send_attempts: AtomicUsize::new(0),
                close_notify: Notify::new(),
            }
        }

        fn sent_datagrams(&self) -> Vec<Vec<u8>> {
            self.sent_datagrams
                .lock()
                .expect("sent_datagrams lock should succeed")
                .clone()
        }

        fn send_attempts(&self) -> usize {
            self.send_attempts.load(Ordering::SeqCst)
        }

        fn sent_payload_ptrs(&self) -> Vec<usize> {
            self.sent_payload_ptrs
                .lock()
                .expect("sent_payload_ptrs lock should succeed")
                .clone()
        }
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
            self.max_datagram_size
        }

        fn send_datagram(&self, payload: Bytes) -> Result<(), ImagodError> {
            self.send_attempts.fetch_add(1, Ordering::SeqCst);
            self.sent_payload_ptrs
                .lock()
                .expect("sent_payload_ptrs lock should succeed")
                .push(payload.as_ptr() as usize);
            self.sent_datagrams
                .lock()
                .expect("sent_datagrams lock should succeed")
                .push(payload.to_vec());

            let outcome = self
                .send_outcomes
                .lock()
                .expect("send_outcomes lock should succeed")
                .pop_front()
                .unwrap_or(Ok(()));
            match outcome {
                Ok(()) => Ok(()),
                Err(message) => Err(ImagodError::new(
                    ErrorCode::Internal,
                    "logs.datagram",
                    message,
                )),
            }
        }

        fn peer_identity(&self) -> Option<Box<dyn Any>> {
            None
        }

        async fn closed(&self) {
            self.close_notify.notified().await;
        }
    }

    fn sample_subscription(name: &str, snapshot_bytes: &[u8]) -> ServiceLogSubscription {
        ServiceLogSubscription {
            service_name: name.to_string(),
            snapshot: ServiceLogSnapshot::Bytes(snapshot_bytes.to_vec()),
            receiver: None,
        }
    }

    #[tokio::test]
    async fn given_retryable_datagram_error__when_send_datagram_with_retry__then_second_attempt_succeeds()
     {
        let session =
            FakeProtocolSession::new(1200, vec![Err("first failure".to_string()), Ok(())]);

        send_datagram_with_retry(&session, Bytes::from(vec![0x01, 0x02]))
            .await
            .expect("second attempt should succeed");
        assert_eq!(session.send_attempts(), 2);
    }

    #[tokio::test]
    async fn given_datagram_send_failures__when_send_datagram_with_retry__then_last_error_is_returned()
     {
        let session = FakeProtocolSession::new(
            1200,
            vec![
                Err("e1".to_string()),
                Err("e2".to_string()),
                Err("e3".to_string()),
                Err("e4".to_string()),
            ],
        );

        let err = send_datagram_with_retry(&session, Bytes::from(vec![0x0a]))
            .await
            .expect_err("all attempts should fail");
        assert_eq!(session.send_attempts(), 4);
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "logs.datagram");
        assert!(err.message.contains("e4"));
    }

    #[tokio::test]
    async fn given_retry_send__when_send_datagram_with_retry__then_payload_uses_shared_backing_buffer()
     {
        let session =
            FakeProtocolSession::new(1200, vec![Err("first failure".to_string()), Ok(())]);
        send_datagram_with_retry(&session, Bytes::from(vec![0xaa; 32]))
            .await
            .expect("second attempt should succeed");

        let ptrs = session.sent_payload_ptrs();
        assert_eq!(ptrs.len(), 2);
        assert_eq!(ptrs[0], ptrs[1]);
    }

    #[tokio::test]
    async fn given_small_datagram_limit__when_send_datagram_envelope__then_internal_size_error_is_returned()
     {
        let session = FakeProtocolSession::new(64, vec![Ok(())]);
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let chunk = LogChunk {
            request_id,
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Composite,
            bytes: vec![0x41; 512],
            is_last: false,
            timestamp_unix_ms: None,
        };
        let envelope =
            ProtocolEnvelope::new(MessageType::LogsChunk, request_id, correlation_id, chunk);

        let err = send_datagram_envelope(&session, &envelope, 64)
            .await
            .expect_err("payload larger than max datagram should fail");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "logs.datagram");
        assert!(err.message.contains("datagram payload too large"));
    }

    #[tokio::test]
    async fn given_snapshot_subscription__when_run_logs_forwarder__then_chunk_and_end_datagrams_are_emitted()
     {
        let session = Arc::new(FakeProtocolSession::new(2048, vec![Ok(()), Ok(()), Ok(())]));
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let subscriptions = vec![sample_subscription("svc-a", b"hello-log")];

        run_logs_forwarder(
            session.clone(),
            request_id,
            correlation_id,
            subscriptions,
            false,
        )
        .await;

        let sent = session.sent_datagrams();
        assert_eq!(sent.len(), 3, "snapshot chunk + terminal chunk + logs.end");

        let first = from_cbor::<ProtocolEnvelope<LogChunk>>(&sent[0]).expect("first chunk decode");
        assert_eq!(first.message_type, MessageType::LogsChunk);
        assert_eq!(first.payload.name, "svc-a");
        assert_eq!(first.payload.bytes, b"hello-log".to_vec());
        assert!(!first.payload.is_last);
        assert_eq!(first.payload.timestamp_unix_ms, None);

        let second =
            from_cbor::<ProtocolEnvelope<LogChunk>>(&sent[1]).expect("second chunk decode");
        assert_eq!(second.message_type, MessageType::LogsChunk);
        assert_eq!(second.payload.name, "svc-a");
        assert!(second.payload.bytes.is_empty());
        assert!(second.payload.is_last);

        let end = from_cbor::<ProtocolEnvelope<LogEnd>>(&sent[2]).expect("logs.end decode");
        assert_eq!(end.message_type, MessageType::LogsEnd);
        assert_eq!(end.payload.request_id, request_id);
        assert!(end.payload.error.is_none());
    }

    #[tokio::test]
    async fn given_large_snapshot_bytes__when_run_logs_forwarder__then_all_bytes_are_chunked_and_forwarded()
     {
        let session = Arc::new(FakeProtocolSession::new(2048, Vec::new()));
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let mut snapshot = Vec::with_capacity(96 * 1024);
        for idx in 0..(96 * 1024) {
            snapshot.push(if idx % 97 == 0 { b'\n' } else { b'a' });
        }
        let subscriptions = vec![sample_subscription("svc-large", &snapshot)];

        run_logs_forwarder(
            session.clone(),
            request_id,
            correlation_id,
            subscriptions,
            false,
        )
        .await;

        let sent = session.sent_datagrams();
        assert!(
            sent.len() > 3,
            "large payload should produce multiple chunks"
        );

        let mut forwarded = Vec::new();
        for datagram in sent.iter().take(sent.len().saturating_sub(1)) {
            let chunk =
                from_cbor::<ProtocolEnvelope<LogChunk>>(datagram).expect("chunk should decode");
            if !chunk.payload.is_last {
                forwarded.extend_from_slice(&chunk.payload.bytes);
            }
        }

        assert_eq!(forwarded, snapshot);
        let end = from_cbor::<ProtocolEnvelope<LogEnd>>(
            sent.last().expect("logs.end datagram should exist"),
        )
        .expect("logs.end decode");
        assert_eq!(end.message_type, MessageType::LogsEnd);
    }

    #[tokio::test]
    async fn given_too_small_datagram_capacity__when_run_logs_forwarder__then_forwarding_aborts_without_datagram()
     {
        let session = Arc::new(FakeProtocolSession::new(1, vec![Ok(())]));
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let subscriptions = vec![sample_subscription("svc-a", b"x")];

        run_logs_forwarder(
            session.clone(),
            request_id,
            correlation_id,
            subscriptions,
            true,
        )
        .await;

        let sent = session.sent_datagrams();
        assert!(
            sent.is_empty(),
            "max_datagram_size=1 cannot encode even logs.end envelope"
        );
        assert_eq!(session.send_attempts(), 0);
    }

    #[test]
    fn given_directional_stream_and_error_code__when_mapping_helpers__then_protocol_values_are_stable()
     {
        assert_eq!(
            service_log_stream_to_protocol(ServiceLogStream::Stdout),
            LogStreamKind::Stdout
        );
        assert_eq!(
            service_log_stream_to_protocol(ServiceLogStream::Stderr),
            LogStreamKind::Stderr
        );

        let not_found = ImagodError::new(ErrorCode::NotFound, "logs.request", "missing");
        assert_eq!(
            log_error_from_imagod_error(&not_found).code,
            LogErrorCode::ProcessNotFound
        );
        let unauthorized = ImagodError::new(ErrorCode::Unauthorized, "logs.request", "denied");
        assert_eq!(
            log_error_from_imagod_error(&unauthorized).code,
            LogErrorCode::PermissionDenied
        );
        let internal = ImagodError::new(ErrorCode::Internal, "logs.request", "oops");
        assert_eq!(
            log_error_from_imagod_error(&internal).code,
            LogErrorCode::Internal
        );
    }

    #[test]
    fn given_lagged_event_count__when_advance_seq_for_lagged__then_seq_uses_saturating_add() {
        let mut seq = u64::MAX - 1;
        advance_seq_for_lagged(&mut seq, 10);
        assert_eq!(seq, u64::MAX);
    }

    #[test]
    fn given_datagram_budget_and_service_names__when_fixed_log_chunk_size__then_limits_and_errors_follow_contract()
     {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let names = vec!["svc-a".to_string(), "service-with-longer-name".to_string()];
        let chunk_size = fixed_log_chunk_size(request_id, correlation_id, 2048, &names, true)
            .expect("chunk size should be computed");
        assert!(chunk_size > 0);
        assert!(chunk_size <= LOG_DATAGRAM_TARGET_BYTES);

        let err = fixed_log_chunk_size(request_id, correlation_id, 1, &names, false)
            .expect_err("too-small datagram budget should fail");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "logs.datagram");
        assert!(err.message.contains("too small"));
    }

    #[tokio::test]
    async fn given_empty_subscriptions__when_run_logs_forwarder__then_no_datagram_is_sent() {
        let session = Arc::new(FakeProtocolSession::new(1200, vec![Ok(())]));
        run_logs_forwarder(
            session.clone(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Vec::new(),
            false,
        )
        .await;
        assert!(
            session.sent_datagrams().is_empty(),
            "no subscriptions should skip forwarding"
        );
    }

    #[tokio::test]
    async fn given_follow_receiver_with_lagged_events__when_stream_logs_datagrams__then_seq_advances_and_events_are_forwarded()
     {
        let session = FakeProtocolSession::new(2048, vec![Ok(()), Ok(()), Ok(())]);
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let sender = LogsDatagramSender::new(&session, request_id, correlation_id, 2048, 512, true);
        let mut seq = 0u64;
        let mut last_name = None;

        let (tx, rx) = broadcast::channel::<ServiceLogEvent>(1);
        tx.send(ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"first".to_vec(),
            timestamp_unix_ms: 10,
        })
        .expect("first send should succeed");
        tx.send(ServiceLogEvent {
            stream: ServiceLogStream::Stderr,
            bytes: b"second".to_vec(),
            timestamp_unix_ms: 11,
        })
        .expect("second send should succeed");
        drop(tx);

        let subscriptions = vec![ServiceLogSubscription {
            service_name: "svc-follow".to_string(),
            snapshot: ServiceLogSnapshot::Bytes(Vec::new()),
            receiver: Some(rx),
        }];
        stream_logs_datagrams(&session, &sender, subscriptions, &mut seq, &mut last_name)
            .await
            .expect("streaming should succeed");

        assert!(
            seq >= 2,
            "lagged + at least one forwarded event should advance sequence"
        );
        assert_eq!(last_name.as_deref(), Some("svc-follow"));
    }
}
