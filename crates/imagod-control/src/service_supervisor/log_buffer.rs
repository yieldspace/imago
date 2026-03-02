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
    total_bytes: usize,
    front_offset: usize,
    chunks: VecDeque<Vec<u8>>,
}

impl BoundedLogBuffer {
    /// Creates a new bounded log buffer.
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.max(1),
            total_bytes: 0,
            front_offset: 0,
            chunks: VecDeque::new(),
        }
    }

    fn evict_front_bytes(&mut self, mut bytes_to_evict: usize) {
        while bytes_to_evict > 0 {
            let Some(front) = self.chunks.front() else {
                break;
            };
            let front_len = front.len().saturating_sub(self.front_offset);
            if front_len <= bytes_to_evict {
                bytes_to_evict = bytes_to_evict.saturating_sub(front_len);
                self.total_bytes = self.total_bytes.saturating_sub(front_len);
                let _ = self.chunks.pop_front();
                self.front_offset = 0;
                continue;
            }
            self.front_offset = self.front_offset.saturating_add(bytes_to_evict);
            self.total_bytes = self.total_bytes.saturating_sub(bytes_to_evict);
            bytes_to_evict = 0;
        }

        if self.chunks.is_empty() {
            self.front_offset = 0;
            self.total_bytes = 0;
        }
    }

    /// Appends bytes and evicts oldest data when capacity is exceeded.
    pub(super) fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }

        if chunk.len() >= self.max_bytes {
            self.chunks.clear();
            self.front_offset = 0;
            let start = chunk.len().saturating_sub(self.max_bytes);
            self.chunks.push_back(chunk[start..].to_vec());
            self.total_bytes = self.max_bytes;
            return;
        }

        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        self.chunks.push_back(chunk.to_vec());
        if self.total_bytes > self.max_bytes {
            self.evict_front_bytes(self.total_bytes.saturating_sub(self.max_bytes));
        }
    }

    pub(super) fn bytes_from_offset(&self, offset: usize) -> Vec<u8> {
        if offset >= self.total_bytes {
            return Vec::new();
        }
        let mut remaining_skip = offset;
        let mut out = Vec::with_capacity(self.total_bytes.saturating_sub(offset));
        for (index, chunk) in self.chunks.iter().enumerate() {
            let segment = if index == 0 {
                &chunk[self.front_offset..]
            } else {
                chunk.as_slice()
            };
            if remaining_skip >= segment.len() {
                remaining_skip = remaining_skip.saturating_sub(segment.len());
                continue;
            }
            out.extend_from_slice(&segment[remaining_skip..]);
            remaining_skip = 0;
        }
        out
    }

    pub(super) fn tail_start_offset_by_lines(&self, tail_lines: u32) -> usize {
        if tail_lines == 0 || self.total_bytes == 0 {
            return self.total_bytes;
        }

        let keep_lines = tail_lines as usize;
        let mut recent_line_starts = VecDeque::with_capacity(keep_lines.max(1));
        recent_line_starts.push_back(0usize);
        let mut total_line_starts = 1usize;
        let mut index = 0usize;

        for (chunk_index, chunk) in self.chunks.iter().enumerate() {
            let segment = if chunk_index == 0 {
                &chunk[self.front_offset..]
            } else {
                chunk.as_slice()
            };
            for byte in segment {
                if *byte == b'\n' && index + 1 < self.total_bytes {
                    total_line_starts = total_line_starts.saturating_add(1);
                    recent_line_starts.push_back(index + 1);
                    if recent_line_starts.len() > keep_lines {
                        let _ = recent_line_starts.pop_front();
                    }
                }
                index = index.saturating_add(1);
            }
        }

        if total_line_starts <= keep_lines {
            0
        } else {
            recent_line_starts.front().copied().unwrap_or(0)
        }
    }

    pub(super) fn tail_lines(&self, tail_lines: u32) -> Vec<u8> {
        let offset = self.tail_start_offset_by_lines(tail_lines);
        self.bytes_from_offset(offset)
    }

    pub(super) fn snapshot(&self) -> Vec<u8> {
        self.bytes_from_offset(0)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.total_bytes
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

    fn evict_front_bytes(&mut self, mut bytes_to_evict: usize) {
        while bytes_to_evict > 0 {
            let Some(front) = self.events.front_mut() else {
                break;
            };
            let front_len = front.bytes.len();
            if front_len <= bytes_to_evict {
                bytes_to_evict = bytes_to_evict.saturating_sub(front_len);
                self.total_bytes = self.total_bytes.saturating_sub(front_len);
                let _ = self.events.pop_front();
                continue;
            }

            let trimmed = front.bytes.split_off(bytes_to_evict);
            front.bytes = trimmed;
            self.total_bytes = self.total_bytes.saturating_sub(bytes_to_evict);
            bytes_to_evict = 0;
        }
    }

    /// Appends one event and evicts oldest bytes when capacity is exceeded.
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
        if self.total_bytes > self.max_bytes {
            self.evict_front_bytes(self.total_bytes.saturating_sub(self.max_bytes));
        }
    }

    pub(super) fn snapshot(&self) -> Vec<ServiceLogEvent> {
        self.events.iter().cloned().collect()
    }

    pub(super) fn tail_from_offset(
        &self,
        mut offset: usize,
        tailed_bytes: &[u8],
    ) -> Vec<ServiceLogEvent> {
        if tailed_bytes.is_empty() {
            return Vec::new();
        }

        let mut remaining = tailed_bytes.len();
        let mut out = Vec::new();
        for event in &self.events {
            if remaining == 0 {
                break;
            }
            if offset >= event.bytes.len() {
                offset = offset.saturating_sub(event.bytes.len());
                continue;
            }
            let start = offset;
            let available = event.bytes.len().saturating_sub(start);
            let take = available.min(remaining);
            if take > 0 {
                out.push(ServiceLogEvent {
                    stream: event.stream,
                    bytes: event.bytes[start..start + take].to_vec(),
                    timestamp_unix_ms: event.timestamp_unix_ms,
                });
                remaining = remaining.saturating_sub(take);
            }
            offset = 0;
        }

        if remaining == 0 {
            return out;
        }

        vec![ServiceLogEvent {
            stream: self
                .events
                .back()
                .map(|event| event.stream)
                .unwrap_or(ServiceLogStream::Stdout),
            bytes: tailed_bytes.to_vec(),
            timestamp_unix_ms: self
                .events
                .back()
                .map(|event| event.timestamp_unix_ms)
                .unwrap_or(0),
        }]
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.events.len()
    }
}

#[derive(Debug)]
/// Bounded composite buffer keeping bytes and timestamped events in the same order.
pub(super) struct CompositeLogBuffer {
    bytes: BoundedLogBuffer,
    events: BoundedLogEventBuffer,
}

impl CompositeLogBuffer {
    /// Creates a new bounded composite log buffer.
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            bytes: BoundedLogBuffer::new(max_bytes),
            events: BoundedLogEventBuffer::new(max_bytes),
        }
    }

    /// Appends one log event to both byte and event rings in one call.
    pub(super) fn push_event(&mut self, event: ServiceLogEvent) {
        self.bytes.push(&event.bytes);
        self.events.push(event);
    }

    pub(super) fn snapshot(&self) -> (Vec<u8>, Vec<ServiceLogEvent>) {
        (self.bytes.snapshot(), self.events.snapshot())
    }

    pub(super) fn tail_snapshot(&self, tail_lines: u32) -> (Vec<u8>, Vec<ServiceLogEvent>) {
        let start_offset = self.bytes.tail_start_offset_by_lines(tail_lines);
        let tailed_bytes = self.bytes.tail_lines(tail_lines);
        let tailed_events = self.events.tail_from_offset(start_offset, &tailed_bytes);
        (tailed_bytes, tailed_events)
    }
}

/// Drains one child output stream into bounded in-memory log buffer.
///
/// Concurrency: runs as a detached task per stream.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_log_drain<R>(
    mut reader: R,
    composite_buffer: Arc<Mutex<CompositeLogBuffer>>,
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
            let timestamp_unix_ms = unix_timestamp_ms_now();
            let event = ServiceLogEvent {
                stream,
                bytes: chunk[..read].to_vec(),
                timestamp_unix_ms,
            };
            {
                let mut guard = composite_buffer.lock().await;
                guard.push_event(event.clone());
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
        let composite = Arc::new(Mutex::new(CompositeLogBuffer::new(256)));
        let (sender, _) = broadcast::channel(16);
        let mut receiver = sender.subscribe();
        let (mut writer, reader) = duplex(256);

        spawn_log_drain(
            reader,
            composite.clone(),
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
                let (composite_snapshot, composite_events_snapshot) =
                    { composite.lock().await.snapshot() };
                if composite_snapshot == expected && !composite_events_snapshot.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("log drain should publish snapshots");
    }

    #[test]
    fn composite_log_buffer_snapshot_matches_joined_event_bytes() {
        let mut buffer = CompositeLogBuffer::new(8);
        buffer.push_event(ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"ab".to_vec(),
            timestamp_unix_ms: 1,
        });
        buffer.push_event(ServiceLogEvent {
            stream: ServiceLogStream::Stderr,
            bytes: b"cd".to_vec(),
            timestamp_unix_ms: 2,
        });

        let (snapshot_bytes, snapshot_events) = buffer.snapshot();
        assert_eq!(
            snapshot_events
                .iter()
                .flat_map(|event| event.bytes.clone())
                .collect::<Vec<_>>(),
            snapshot_bytes
        );
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
        assert_eq!(
            snapshot
                .iter()
                .flat_map(|event| event.bytes.clone())
                .collect::<Vec<_>>(),
            b"bcdef"
        );
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn bounded_log_buffer_evicts_with_chunk_aware_front_offset() {
        let mut buffer = BoundedLogBuffer::new(5);
        buffer.push(b"abc");
        buffer.push(b"def");

        assert_eq!(buffer.snapshot(), b"bcdef");
        assert_eq!(buffer.len(), 5);
        assert_eq!(buffer.front_offset, 1);
        assert_eq!(buffer.chunks.len(), 2);
    }

    #[test]
    fn bounded_log_buffer_large_chunk_keeps_latest_tail_only() {
        let mut buffer = BoundedLogBuffer::new(4);
        buffer.push(b"0123456789");

        assert_eq!(buffer.snapshot(), b"6789");
        assert_eq!(buffer.front_offset, 0);
        assert_eq!(buffer.chunks.len(), 1);
    }

    #[test]
    fn composite_log_buffer_tail_snapshot_aligns_bytes_and_events() {
        let mut buffer = CompositeLogBuffer::new(32);
        buffer.push_event(ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"line-1\n".to_vec(),
            timestamp_unix_ms: 1,
        });
        buffer.push_event(ServiceLogEvent {
            stream: ServiceLogStream::Stderr,
            bytes: b"line-2\n".to_vec(),
            timestamp_unix_ms: 2,
        });
        buffer.push_event(ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"line-3\n".to_vec(),
            timestamp_unix_ms: 3,
        });

        let (tail_bytes, tail_events) = buffer.tail_snapshot(2);
        assert_eq!(tail_bytes, b"line-2\nline-3\n");
        assert_eq!(
            tail_events
                .iter()
                .flat_map(|event| event.bytes.clone())
                .collect::<Vec<_>>(),
            tail_bytes
        );
        assert_eq!(tail_events[0].stream, ServiceLogStream::Stderr);
        assert_eq!(tail_events[1].stream, ServiceLogStream::Stdout);
    }
}
