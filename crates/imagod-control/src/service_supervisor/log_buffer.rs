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

    #[cfg(test)]
    pub(super) fn snapshot_bytes(&self) -> Vec<u8> {
        materialize_bytes_from_offset(self.events.iter(), 0, self.total_bytes)
    }

    pub(super) fn tail_snapshot_events(&self, tail_lines: u32) -> Vec<ServiceLogEvent> {
        let start_offset =
            compute_tail_start_offset_by_lines(self.events.iter(), self.total_bytes, tail_lines);
        slice_events_from_offset(
            self.events.iter(),
            start_offset,
            self.total_bytes.saturating_sub(start_offset),
        )
    }

    pub(super) fn tail_snapshot_bytes(&self, tail_lines: u32) -> Vec<u8> {
        let start_offset =
            compute_tail_start_offset_by_lines(self.events.iter(), self.total_bytes, tail_lines);
        materialize_bytes_from_offset(
            self.events.iter(),
            start_offset,
            self.total_bytes.saturating_sub(start_offset),
        )
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    pub(super) fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

#[derive(Debug)]
/// Bounded composite buffer keeping one payload-owning event ring.
pub(super) struct CompositeLogBuffer {
    events: BoundedLogEventBuffer,
}

impl CompositeLogBuffer {
    /// Creates a new bounded composite log buffer.
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            events: BoundedLogEventBuffer::new(max_bytes),
        }
    }

    /// Appends one log event to the bounded event ring.
    pub(super) fn push_event(&mut self, event: ServiceLogEvent) {
        self.events.push(event);
    }

    #[cfg(test)]
    pub(super) fn snapshot_bytes(&self) -> Vec<u8> {
        self.events.snapshot_bytes()
    }

    pub(super) fn snapshot_events(&self) -> Vec<ServiceLogEvent> {
        self.events.snapshot()
    }

    pub(super) fn tail_snapshot_bytes(&self, tail_lines: u32) -> Vec<u8> {
        self.events.tail_snapshot_bytes(tail_lines)
    }

    pub(super) fn tail_snapshot_events(&self, tail_lines: u32) -> Vec<ServiceLogEvent> {
        self.events.tail_snapshot_events(tail_lines)
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

#[cfg(test)]
pub(super) fn snapshot_bytes_from_events(events: &[ServiceLogEvent]) -> Vec<u8> {
    materialize_bytes_from_offset(events.iter(), 0, total_bytes_for_events(events))
}

pub(super) fn tail_snapshot_events_from_events(
    events: &[ServiceLogEvent],
    tail_lines: u32,
) -> Vec<ServiceLogEvent> {
    let total_bytes = total_bytes_for_events(events);
    let start_offset = compute_tail_start_offset_by_lines(events.iter(), total_bytes, tail_lines);
    slice_events_from_offset(
        events.iter(),
        start_offset,
        total_bytes.saturating_sub(start_offset),
    )
}

pub(super) fn tail_snapshot_bytes_from_events(
    events: &[ServiceLogEvent],
    tail_lines: u32,
) -> Vec<u8> {
    let total_bytes = total_bytes_for_events(events);
    let start_offset = compute_tail_start_offset_by_lines(events.iter(), total_bytes, tail_lines);
    materialize_bytes_from_offset(
        events.iter(),
        start_offset,
        total_bytes.saturating_sub(start_offset),
    )
}

fn total_bytes_for_events(events: &[ServiceLogEvent]) -> usize {
    events.iter().map(|event| event.bytes.len()).sum()
}

fn compute_tail_start_offset_by_lines<'a, I>(
    events: I,
    total_bytes: usize,
    tail_lines: u32,
) -> usize
where
    I: IntoIterator<Item = &'a ServiceLogEvent>,
{
    if tail_lines == 0 || total_bytes == 0 {
        return total_bytes;
    }

    let keep_lines = tail_lines as usize;
    let tracked_line_starts_limit = keep_lines.min(total_bytes.saturating_add(1)).max(1);
    let mut recent_line_starts = VecDeque::with_capacity(tracked_line_starts_limit);
    recent_line_starts.push_back(0usize);
    let mut total_line_starts = 1usize;
    let mut index = 0usize;

    for event in events {
        for byte in &event.bytes {
            if *byte == b'\n' && index + 1 < total_bytes {
                total_line_starts = total_line_starts.saturating_add(1);
                recent_line_starts.push_back(index + 1);
                if recent_line_starts.len() > tracked_line_starts_limit {
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

fn materialize_bytes_from_offset<'a, I>(events: I, mut offset: usize, remaining: usize) -> Vec<u8>
where
    I: IntoIterator<Item = &'a ServiceLogEvent>,
{
    if remaining == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(remaining);
    let mut remaining = remaining;
    for event in events {
        if remaining == 0 {
            break;
        }
        if offset >= event.bytes.len() {
            offset = offset.saturating_sub(event.bytes.len());
            continue;
        }

        let start = offset;
        let take = event.bytes.len().saturating_sub(start).min(remaining);
        out.extend_from_slice(&event.bytes[start..start + take]);
        offset = 0;
        remaining = remaining.saturating_sub(take);
    }

    out
}

fn slice_events_from_offset<'a, I>(
    events: I,
    mut offset: usize,
    mut remaining: usize,
) -> Vec<ServiceLogEvent>
where
    I: IntoIterator<Item = &'a ServiceLogEvent>,
{
    if remaining == 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for event in events {
        if remaining == 0 {
            break;
        }
        if offset >= event.bytes.len() {
            offset = offset.saturating_sub(event.bytes.len());
            continue;
        }

        let start = offset;
        let take = event.bytes.len().saturating_sub(start).min(remaining);
        if take > 0 {
            out.push(ServiceLogEvent {
                stream: event.stream,
                bytes: event.bytes[start..start + take].to_vec(),
                timestamp_unix_ms: event.timestamp_unix_ms,
            });
        }
        offset = 0;
        remaining = remaining.saturating_sub(take);
    }

    out
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
                let (composite_snapshot, composite_events_snapshot) = {
                    let guard = composite.lock().await;
                    (guard.snapshot_bytes(), guard.snapshot_events())
                };
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

        let snapshot_bytes = buffer.snapshot_bytes();
        let snapshot_events = buffer.snapshot_events();
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
        assert_eq!(buffer.total_bytes(), 5);
    }

    #[test]
    fn snapshot_bytes_from_events_preserves_joined_bytes() {
        let events = vec![
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"abc".to_vec(),
                timestamp_unix_ms: 1,
            },
            ServiceLogEvent {
                stream: ServiceLogStream::Stderr,
                bytes: b"def".to_vec(),
                timestamp_unix_ms: 2,
            },
        ];

        assert_eq!(snapshot_bytes_from_events(&events), b"abcdef");
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

        let tail_bytes = buffer.tail_snapshot_bytes(2);
        let tail_events = buffer.tail_snapshot_events(2);
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

    #[test]
    fn tail_snapshot_bytes_from_events_with_huge_request_returns_full_snapshot() {
        let events = vec![ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"line-1\nline-2\nline-3".to_vec(),
            timestamp_unix_ms: 1,
        }];

        assert_eq!(
            tail_snapshot_bytes_from_events(&events, u32::MAX),
            snapshot_bytes_from_events(&events)
        );
    }
}
