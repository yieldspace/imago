use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use bytes::Bytes;
use imagod_common::ImagodError;
use imagod_control::{ServiceLogSnapshot, ServiceLogSubscription};
use tokio::sync::Notify;
use uuid::Uuid;

use super::{
    logs_forwarder::{run_logs_forwarder, send_datagram_with_retry},
    session_loop::ProtocolSession,
};

struct BenchProtocolSession {
    max_datagram_size: usize,
    fail_attempts_remaining: AtomicUsize,
    send_attempts: AtomicUsize,
    sent_datagrams: AtomicUsize,
    close_notify: Notify,
}

impl BenchProtocolSession {
    fn new(max_datagram_size: usize, fail_attempts: usize) -> Self {
        Self {
            max_datagram_size,
            fail_attempts_remaining: AtomicUsize::new(fail_attempts),
            send_attempts: AtomicUsize::new(0),
            sent_datagrams: AtomicUsize::new(0),
            close_notify: Notify::new(),
        }
    }

    fn send_attempts(&self) -> usize {
        self.send_attempts.load(Ordering::SeqCst)
    }

    fn sent_datagrams(&self) -> usize {
        self.sent_datagrams.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ProtocolSession for BenchProtocolSession {
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

    fn send_datagram(&self, _payload: Bytes) -> Result<(), ImagodError> {
        self.send_attempts.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_attempts_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                (remaining > 0).then_some(remaining - 1)
            })
            .is_ok()
        {
            return Err(ImagodError::new(
                imago_protocol::ErrorCode::Internal,
                "logs.datagram",
                "bench injected send failure",
            ));
        }
        self.sent_datagrams.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn peer_identity(&self) -> Option<Box<dyn Any>> {
        None
    }

    async fn closed(&self) {
        self.close_notify.notified().await;
    }
}

/// Runs one retry send path and returns the number of send attempts.
pub async fn bench_retry_send_attempts(
    payload_len: usize,
    fail_attempts: usize,
) -> Result<usize, ImagodError> {
    let session = BenchProtocolSession::new(2048, fail_attempts);
    let payload = Bytes::from(vec![0x5a; payload_len.max(1)]);
    send_datagram_with_retry(&session, payload).await?;
    Ok(session.send_attempts())
}

/// Runs one large snapshot forwarding path and returns emitted datagram count.
pub async fn bench_forward_snapshot_datagrams(snapshot_len: usize) -> usize {
    let session = Arc::new(BenchProtocolSession::new(2048, 0));
    let request_id = Uuid::new_v4();
    let correlation_id = Uuid::new_v4();
    let mut snapshot = vec![b'a'; snapshot_len.max(1)];
    for idx in (0..snapshot.len()).step_by(89) {
        snapshot[idx] = b'\n';
    }
    let subscriptions = vec![ServiceLogSubscription {
        service_name: "svc-bench".to_string(),
        snapshot: ServiceLogSnapshot::Bytes(snapshot),
        receiver: None,
    }];

    run_logs_forwarder(
        session.clone(),
        request_id,
        correlation_id,
        subscriptions,
        false,
    )
    .await;
    session.sent_datagrams()
}
