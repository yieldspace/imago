use std::sync::Arc;

use async_trait::async_trait;
use imago_protocol::{
    LogChunk, LogEnd, LogError, LogErrorCode, LogStreamKind, MessageType, ProtocolEnvelope, to_cbor,
};
use imagod_common::ImagodError;
use imagod_control::{ServiceLogEvent, ServiceLogStream, ServiceLogSubscription};
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
    ) where
        S: ProtocolSession + 'static,
    {
        run_logs_forwarder(session, request_id, correlation_id, subscriptions).await;
    }
}

pub(crate) async fn run_logs_forwarder<S>(
    session: Arc<S>,
    request_id: Uuid,
    correlation_id: Uuid,
    subscriptions: Vec<ServiceLogSubscription>,
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
                    Vec::new(),
                    true,
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
    ) -> Self {
        Self {
            session,
            request_id,
            correlation_id,
            max_datagram_size,
            chunk_size,
        }
    }

    async fn send_log_data_chunks(
        &self,
        seq: &mut u64,
        name: &str,
        stream_kind: LogStreamKind,
        bytes: &[u8],
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
            self.send_single_log_chunk(seq, name, stream_kind, bytes[offset..end].to_vec(), false)
                .await?;
            *last_name = Some(name.to_string());
            offset = end;
        }

        Ok(())
    }

    async fn send_single_log_chunk(
        &self,
        seq: &mut u64,
        name: &str,
        stream_kind: LogStreamKind,
        bytes: Vec<u8>,
        is_last: bool,
    ) -> Result<(), ImagodError> {
        let chunk = LogChunk {
            request_id: self.request_id,
            seq: *seq,
            name: name.to_string(),
            stream_kind,
            bytes,
            is_last,
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
        sender
            .send_log_data_chunks(
                seq,
                &subscription.service_name,
                LogStreamKind::Composite,
                &subscription.snapshot_bytes,
                last_name,
            )
            .await?;
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
    send_datagram_with_retry(session, bytes).await
}

async fn send_datagram_with_retry<S>(session: &S, bytes: Vec<u8>) -> Result<(), ImagodError>
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
