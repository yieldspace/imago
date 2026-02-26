use std::{
    collections::VecDeque,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::{Mutex, broadcast},
};

use super::{ServiceLogEvent, ServiceLogStream};

#[derive(Debug)]
/// Bounded byte ring used for per-stream runner log capture.
pub(super) struct BoundedLogBuffer {
    max_bytes: usize,
    bytes: VecDeque<u8>,
}

impl BoundedLogBuffer {
    /// Creates a new bounded log buffer.
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.max(1),
            bytes: VecDeque::new(),
        }
    }

    /// Appends bytes and evicts oldest data when capacity is exceeded.
    pub(super) fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.bytes.extend(chunk.iter().copied());
        while self.bytes.len() > self.max_bytes {
            let _ = self.bytes.pop_front();
        }
    }

    pub(super) fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Debug)]
/// Bounded event ring used for timestamp-preserving log snapshot capture.
pub(super) struct BoundedLogEventBuffer {
    max_bytes: usize,
    total_bytes: usize,
    events: VecDeque<ServiceLogEvent>,
}

impl BoundedLogEventBuffer {
    /// Creates a new bounded log event buffer.
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.max(1),
            total_bytes: 0,
            events: VecDeque::new(),
        }
    }

    /// Appends one event and evicts oldest events when capacity is exceeded.
    pub(super) fn push(&mut self, mut event: ServiceLogEvent) {
        if event.bytes.is_empty() {
            return;
        }
        if event.bytes.len() > self.max_bytes {
            let start = event.bytes.len().saturating_sub(self.max_bytes);
            event.bytes = event.bytes[start..].to_vec();
        }

        self.total_bytes = self.total_bytes.saturating_add(event.bytes.len());
        self.events.push_back(event);
        while self.total_bytes > self.max_bytes {
            let Some(evicted) = self.events.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.bytes.len());
        }
    }

    pub(super) fn snapshot(&self) -> Vec<ServiceLogEvent> {
        self.events.iter().cloned().collect()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.events.len()
    }
}

/// Drains one child output stream into bounded in-memory log buffer.
///
/// Concurrency: runs as a detached task per stream.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_log_drain<R>(
    mut reader: R,
    buffer: Arc<Mutex<BoundedLogBuffer>>,
    composite_buffer: Arc<Mutex<BoundedLogBuffer>>,
    composite_events: Arc<Mutex<BoundedLogEventBuffer>>,
    sender: broadcast::Sender<ServiceLogEvent>,
    service_name: String,
    stream_name: &'static str,
    stream: ServiceLogStream,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = vec![0u8; 8192];
        loop {
            let read = match reader.read(&mut chunk).await {
                Ok(v) => v,
                Err(err) => {
                    eprintln!(
                        "service log read error name={} stream={} error={}",
                        service_name, stream_name, err
                    );
                    break;
                }
            };
            if read == 0 {
                break;
            }
            {
                let mut guard = buffer.lock().await;
                guard.push(&chunk[..read]);
            }
            {
                let mut guard = composite_buffer.lock().await;
                guard.push(&chunk[..read]);
            }
            let timestamp_unix_ms = unix_timestamp_ms_now();
            let event = ServiceLogEvent {
                stream,
                bytes: chunk[..read].to_vec(),
                timestamp_unix_ms,
            };
            {
                let mut guard = composite_events.lock().await;
                guard.push(event.clone());
            }
            let _ = sender.send(event);
        }
    });
}

fn unix_timestamp_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(super) fn tail_lines_from_bytes(bytes: &[u8], tail_lines: u32) -> Vec<u8> {
    if tail_lines == 0 || bytes.is_empty() {
        return Vec::new();
    }

    let mut line_starts = vec![0usize];
    for (idx, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' && idx + 1 < bytes.len() {
            line_starts.push(idx + 1);
        }
    }

    if tail_lines as usize >= line_starts.len() {
        return bytes.to_vec();
    }
    let start = line_starts[line_starts.len() - tail_lines as usize];
    bytes[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncWriteExt, duplex};

    use super::*;

    #[tokio::test]
    async fn spawn_log_drain_updates_buffers_and_broadcast_stream() {
        let stream_bytes = b"hello-from-wasm\nline-2\n";
        let expected = stream_bytes.to_vec();
        let per_stream = Arc::new(Mutex::new(BoundedLogBuffer::new(256)));
        let composite = Arc::new(Mutex::new(BoundedLogBuffer::new(256)));
        let composite_events = Arc::new(Mutex::new(BoundedLogEventBuffer::new(256)));
        let (sender, _) = broadcast::channel(16);
        let mut receiver = sender.subscribe();
        let (mut writer, reader) = duplex(256);

        spawn_log_drain(
            reader,
            per_stream.clone(),
            composite.clone(),
            composite_events.clone(),
            sender,
            "svc-test".to_string(),
            "stdout",
            ServiceLogStream::Stdout,
        );

        writer
            .write_all(stream_bytes)
            .await
            .expect("write to in-memory stream should succeed");
        writer
            .shutdown()
            .await
            .expect("shutdown on in-memory stream should succeed");
        drop(writer);

        let mut received_bytes = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_millis(100), receiver.recv()).await {
                Ok(Ok(event)) => {
                    assert_eq!(event.stream, ServiceLogStream::Stdout);
                    assert!(event.timestamp_unix_ms > 0);
                    received_bytes.extend_from_slice(&event.bytes);
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Err(_) => break,
            }
        }
        assert_eq!(received_bytes, expected);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let per_stream_snapshot = { per_stream.lock().await.snapshot() };
                let composite_snapshot = { composite.lock().await.snapshot() };
                let composite_events_snapshot = { composite_events.lock().await.snapshot() };
                if per_stream_snapshot == expected
                    && composite_snapshot == expected
                    && !composite_events_snapshot.is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("log drain should publish snapshots");
    }

    #[test]
    fn bounded_log_event_buffer_keeps_latest_bytes_only() {
        let mut buffer = BoundedLogEventBuffer::new(5);
        buffer.push(ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"abc".to_vec(),
            timestamp_unix_ms: 1,
        });
        buffer.push(ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"def".to_vec(),
            timestamp_unix_ms: 2,
        });

        let snapshot = buffer.snapshot();
        let total_bytes = snapshot
            .iter()
            .map(|event| event.bytes.len())
            .sum::<usize>();
        assert!(total_bytes <= 5);
        assert!(!snapshot.is_empty());
        assert!(buffer.len() >= 1);
    }
}
