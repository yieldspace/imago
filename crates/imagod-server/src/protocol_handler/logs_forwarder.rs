use std::{future::Future, pin::pin};

use imago_protocol::{
    LogChunk, LogEnd, LogError, LogErrorCode, LogStreamKind, MessageType, ProtocolEnvelope, to_cbor,
};
use imagod_common::ImagodError;
use imagod_control::{
    ServiceLogEvent, ServiceLogSnapshot, ServiceLogStream, ServiceLogSubscription,
};
use tokio::{io::AsyncWrite, sync::mpsc};
use uuid::Uuid;

use super::{codec::FrameCodec, envelope_io::bad_request};

const LOG_STREAM_CHUNK_BYTES: usize = 16 * 1024;

pub(crate) struct DefaultLogsForwarder;

impl DefaultLogsForwarder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn forward<W, C>(
        &self,
        send: &mut W,
        request_id: Uuid,
        correlation_id: Uuid,
        subscriptions: Vec<ServiceLogSubscription>,
        with_timestamp: bool,
        close_signal: C,
        frame_codec: &impl FrameCodec,
    ) -> Result<(), ImagodError>
    where
        W: AsyncWrite + Unpin + Send,
        C: Future<Output = ()> + Send,
    {
        run_logs_forwarder(
            send,
            request_id,
            correlation_id,
            subscriptions,
            with_timestamp,
            close_signal,
            frame_codec,
        )
        .await
    }
}

pub(crate) async fn run_logs_forwarder<W, C>(
    send: &mut W,
    request_id: Uuid,
    correlation_id: Uuid,
    subscriptions: Vec<ServiceLogSubscription>,
    with_timestamp: bool,
    close_signal: C,
    frame_codec: &impl FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
    C: Future<Output = ()> + Send,
{
    if subscriptions.is_empty() {
        return Ok(());
    }

    let fallback_name = subscriptions[0].service_name.clone();
    let mut seq = 0u64;
    let mut last_name = None;
    let stream_result = stream_logs_chunks(
        send,
        request_id,
        correlation_id,
        subscriptions,
        with_timestamp,
        &mut seq,
        &mut last_name,
        close_signal,
        frame_codec,
    )
    .await;

    match stream_result {
        Ok(()) => {
            let terminal_name = last_name.unwrap_or(fallback_name);
            send_single_log_chunk(
                send,
                request_id,
                correlation_id,
                &mut seq,
                &terminal_name,
                LogStreamKind::Composite,
                &[],
                true,
                None,
                with_timestamp,
                frame_codec,
            )
            .await?;
            send_logs_end(send, request_id, correlation_id, seq, None, frame_codec).await
        }
        Err(err) => {
            let log_error = log_error_from_imagod_error(&err);
            send_logs_end(
                send,
                request_id,
                correlation_id,
                seq,
                Some(log_error),
                frame_codec,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_logs_chunks<W, C>(
    send: &mut W,
    request_id: Uuid,
    correlation_id: Uuid,
    subscriptions: Vec<ServiceLogSubscription>,
    with_timestamp: bool,
    seq: &mut u64,
    last_name: &mut Option<String>,
    close_signal: C,
    frame_codec: &impl FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
    C: Future<Output = ()> + Send,
{
    for subscription in &subscriptions {
        match &subscription.snapshot {
            ServiceLogSnapshot::Bytes(bytes) => {
                send_log_data_chunks(
                    send,
                    request_id,
                    correlation_id,
                    seq,
                    &subscription.service_name,
                    LogStreamKind::Composite,
                    bytes,
                    None,
                    with_timestamp,
                    last_name,
                    frame_codec,
                )
                .await?;
            }
            ServiceLogSnapshot::Events(events) => {
                if with_timestamp {
                    for event in events {
                        send_log_data_chunks(
                            send,
                            request_id,
                            correlation_id,
                            seq,
                            &subscription.service_name,
                            LogStreamKind::Composite,
                            &event.bytes,
                            Some(event.timestamp_unix_ms),
                            with_timestamp,
                            last_name,
                            frame_codec,
                        )
                        .await?;
                    }
                } else {
                    let bytes = flatten_log_event_bytes(events);
                    send_log_data_chunks(
                        send,
                        request_id,
                        correlation_id,
                        seq,
                        &subscription.service_name,
                        LogStreamKind::Composite,
                        &bytes,
                        None,
                        with_timestamp,
                        last_name,
                        frame_codec,
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

    let mut close_signal = pin!(close_signal);
    loop {
        tokio::select! {
            maybe_msg = rx.recv() => {
                let Some(message) = maybe_msg else {
                    break;
                };
                match message {
                    FollowForwardMsg::Event { service_name, event } => {
                        send_log_data_chunks(
                            send,
                            request_id,
                            correlation_id,
                            seq,
                            &service_name,
                            service_log_stream_to_protocol(event.stream),
                            &event.bytes,
                            Some(event.timestamp_unix_ms),
                            with_timestamp,
                            last_name,
                            frame_codec,
                        )
                        .await?;
                    }
                    FollowForwardMsg::Lagged { service_name, dropped } => {
                        *last_name = Some(service_name);
                        advance_seq_for_lagged(seq, dropped);
                    }
                }
            }
            _ = &mut close_signal => break,
        }
    }

    for task in forward_tasks {
        task.abort();
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_log_data_chunks<W>(
    send: &mut W,
    request_id: Uuid,
    correlation_id: Uuid,
    seq: &mut u64,
    name: &str,
    stream_kind: LogStreamKind,
    bytes: &[u8],
    timestamp_unix_ms: Option<u64>,
    with_timestamp: bool,
    last_name: &mut Option<String>,
    frame_codec: &impl FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
{
    if bytes.is_empty() {
        return Ok(());
    }

    for chunk in bytes.chunks(LOG_STREAM_CHUNK_BYTES) {
        send_single_log_chunk(
            send,
            request_id,
            correlation_id,
            seq,
            name,
            stream_kind,
            chunk,
            false,
            timestamp_unix_ms,
            with_timestamp,
            frame_codec,
        )
        .await?;
    }
    *last_name = Some(name.to_string());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_single_log_chunk<W>(
    send: &mut W,
    request_id: Uuid,
    correlation_id: Uuid,
    seq: &mut u64,
    name: &str,
    stream_kind: LogStreamKind,
    bytes: &[u8],
    is_last: bool,
    timestamp_unix_ms: Option<u64>,
    with_timestamp: bool,
    frame_codec: &impl FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
{
    response_envelope(
        send,
        MessageType::LogsChunk,
        request_id,
        correlation_id,
        LogChunk {
            request_id,
            seq: *seq,
            name: name.to_string(),
            stream_kind,
            bytes: bytes.to_vec(),
            is_last,
            timestamp_unix_ms: if with_timestamp {
                timestamp_unix_ms
            } else {
                None
            },
        },
        frame_codec,
    )
    .await?;
    *seq = seq.saturating_add(1);
    Ok(())
}

async fn send_logs_end<W>(
    send: &mut W,
    request_id: Uuid,
    correlation_id: Uuid,
    seq: u64,
    error: Option<LogError>,
    frame_codec: &impl FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
{
    write_typed_envelope(
        send,
        &ProtocolEnvelope::new(
            MessageType::LogsEnd,
            request_id,
            correlation_id,
            LogEnd {
                request_id,
                seq,
                error,
            },
        ),
        frame_codec,
    )
    .await
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

async fn response_envelope<W, T>(
    send: &mut W,
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    payload: T,
    frame_codec: &impl FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
    T: serde::Serialize,
{
    write_typed_envelope(
        send,
        &ProtocolEnvelope::new(message_type, request_id, correlation_id, payload),
        frame_codec,
    )
    .await
}

async fn write_typed_envelope<W, T>(
    send: &mut W,
    envelope: &ProtocolEnvelope<T>,
    frame_codec: &impl FrameCodec,
) -> Result<(), ImagodError>
where
    W: AsyncWrite + Unpin + Send,
    T: serde::Serialize,
{
    let data = to_cbor(envelope)
        .map_err(|e| bad_request("protocol", format!("cbor encode failed: {e}")))?;
    let framed = frame_codec.encode_frame(&data);
    tokio::io::AsyncWriteExt::write_all(send, &framed)
        .await
        .map_err(|e| {
            ImagodError::new(
                imago_protocol::ErrorCode::Internal,
                "session.write",
                format!("failed to send frame: {e}"),
            )
        })
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    use std::{
        io,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
    };

    use imago_protocol::{ErrorCode, ProtocolEnvelope, from_cbor};
    use imagod_control::{ServiceLogSnapshot, ServiceLogStream, ServiceLogSubscription};
    use tokio::io::AsyncWrite;

    use super::*;
    use crate::protocol_handler::codec::{FrameCodec, LengthPrefixedFrameCodec};

    #[derive(Default)]
    struct CapturedWriteState {
        bytes: Vec<u8>,
        shutdown: bool,
        write_calls: usize,
    }

    #[derive(Clone, Default)]
    struct CapturedWriteStream {
        state: Arc<Mutex<CapturedWriteState>>,
        fail_after_write_call: Option<usize>,
    }

    impl CapturedWriteStream {
        fn new() -> Self {
            Self::default()
        }

        fn with_fail_after_write_call(fail_after_write_call: usize) -> Self {
            Self {
                state: Arc::new(Mutex::new(CapturedWriteState::default())),
                fail_after_write_call: Some(fail_after_write_call),
            }
        }

        fn bytes(&self) -> Vec<u8> {
            self.state
                .lock()
                .expect("captured state lock should succeed")
                .bytes
                .clone()
        }

        fn shutdown_called(&self) -> bool {
            self.state
                .lock()
                .expect("captured state lock should succeed")
                .shutdown
        }
    }

    impl AsyncWrite for CapturedWriteStream {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut state = self
                .state
                .lock()
                .expect("captured state lock should succeed");
            state.write_calls += 1;
            if self
                .fail_after_write_call
                .is_some_and(|fail_after| state.write_calls >= fail_after)
            {
                return Poll::Ready(Err(io::Error::other("forced write failure")));
            }
            state.bytes.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.state
                .lock()
                .expect("captured state lock should succeed")
                .shutdown = true;
            Poll::Ready(Ok(()))
        }
    }

    fn sample_subscription(name: &str, snapshot_bytes: &[u8]) -> ServiceLogSubscription {
        ServiceLogSubscription {
            service_name: name.to_string(),
            snapshot: ServiceLogSnapshot::Bytes(snapshot_bytes.to_vec()),
            receiver: None,
        }
    }

    fn decode_frames(bytes: &[u8]) -> Vec<&[u8]> {
        LengthPrefixedFrameCodec
            .decode_frame_slices(bytes)
            .expect("frame decode should succeed")
    }

    #[tokio::test]
    async fn given_snapshot_subscription__when_run_logs_forwarder__then_chunk_and_end_frames_are_emitted()
     {
        let mut send = CapturedWriteStream::new();
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let subscriptions = vec![sample_subscription("svc-a", b"hello-log")];

        run_logs_forwarder(
            &mut send,
            request_id,
            correlation_id,
            subscriptions,
            false,
            std::future::pending(),
            &LengthPrefixedFrameCodec,
        )
        .await
        .expect("log forwarding should succeed");

        let bytes = send.bytes();
        let frames = decode_frames(&bytes);
        assert_eq!(
            frames.len(),
            3,
            "snapshot chunk + terminal chunk + logs.end"
        );

        let first = from_cbor::<ProtocolEnvelope<LogChunk>>(frames[0]).expect("first chunk decode");
        assert_eq!(first.message_type, MessageType::LogsChunk);
        assert_eq!(first.payload.name, "svc-a");
        assert_eq!(first.payload.bytes, b"hello-log".to_vec());
        assert!(!first.payload.is_last);
        assert_eq!(first.payload.timestamp_unix_ms, None);

        let second =
            from_cbor::<ProtocolEnvelope<LogChunk>>(frames[1]).expect("second chunk decode");
        assert_eq!(second.message_type, MessageType::LogsChunk);
        assert_eq!(second.payload.name, "svc-a");
        assert!(second.payload.bytes.is_empty());
        assert!(second.payload.is_last);

        let end = from_cbor::<ProtocolEnvelope<LogEnd>>(frames[2]).expect("logs.end decode");
        assert_eq!(end.message_type, MessageType::LogsEnd);
        assert_eq!(end.payload.request_id, request_id);
        assert!(end.payload.error.is_none());
    }

    #[tokio::test]
    async fn given_large_snapshot_bytes__when_run_logs_forwarder__then_all_bytes_are_chunked_and_forwarded()
     {
        let mut send = CapturedWriteStream::new();
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let mut snapshot = Vec::with_capacity((LOG_STREAM_CHUNK_BYTES * 3) + 257);
        for idx in 0..snapshot.capacity() {
            snapshot.push(if idx % 97 == 0 { b'\n' } else { b'a' });
        }
        let subscriptions = vec![sample_subscription("svc-large", &snapshot)];

        run_logs_forwarder(
            &mut send,
            request_id,
            correlation_id,
            subscriptions,
            false,
            std::future::pending(),
            &LengthPrefixedFrameCodec,
        )
        .await
        .expect("log forwarding should succeed");

        let bytes = send.bytes();
        let frames = decode_frames(&bytes);
        assert!(
            frames.len() > 3,
            "large payload should produce multiple chunks"
        );

        let mut forwarded = Vec::new();
        for frame in frames.iter().take(frames.len().saturating_sub(1)) {
            let chunk =
                from_cbor::<ProtocolEnvelope<LogChunk>>(frame).expect("chunk should decode");
            if !chunk.payload.is_last {
                forwarded.extend_from_slice(&chunk.payload.bytes);
            }
        }

        assert_eq!(forwarded, snapshot);
        let end = from_cbor::<ProtocolEnvelope<LogEnd>>(
            frames.last().expect("logs.end frame should exist"),
        )
        .expect("logs.end decode");
        assert_eq!(end.message_type, MessageType::LogsEnd);
    }

    #[tokio::test]
    async fn given_empty_subscriptions__when_run_logs_forwarder__then_no_frame_is_written() {
        let mut send = CapturedWriteStream::new();
        run_logs_forwarder(
            &mut send,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Vec::new(),
            false,
            std::future::pending(),
            &LengthPrefixedFrameCodec,
        )
        .await
        .expect("empty subscriptions should be a no-op");
        assert!(
            send.bytes().is_empty(),
            "no subscriptions should skip forwarding"
        );
    }

    #[tokio::test]
    async fn given_write_failure__when_run_logs_forwarder__then_logs_end_error_is_not_written() {
        let mut send = CapturedWriteStream::with_fail_after_write_call(2);
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let subscriptions = vec![sample_subscription("svc-a", b"hello-log")];

        let err = run_logs_forwarder(
            &mut send,
            request_id,
            correlation_id,
            subscriptions,
            false,
            std::future::pending(),
            &LengthPrefixedFrameCodec,
        )
        .await
        .expect_err("write failure should bubble up");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "session.write");
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
}
