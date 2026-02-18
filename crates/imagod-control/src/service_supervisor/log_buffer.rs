use std::{collections::VecDeque, sync::Arc};

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

/// Drains one child output stream into bounded in-memory log buffer.
///
/// Concurrency: runs as a detached task per stream.
pub(super) fn spawn_log_drain<R>(
    mut reader: R,
    buffer: Arc<Mutex<BoundedLogBuffer>>,
    composite_buffer: Arc<Mutex<BoundedLogBuffer>>,
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
            let _ = sender.send(ServiceLogEvent {
                stream,
                bytes: chunk[..read].to_vec(),
            });

            let text = String::from_utf8_lossy(&chunk[..read]);
            for line in text.lines() {
                if line.is_empty() {
                    continue;
                }
                eprintln!(
                    "service log name={} stream={} msg={}",
                    service_name, stream_name, line
                );
            }
        }
    });
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
