use std::{path::Path, time::Duration};

use anyhow::{Context, anyhow};
use imago_protocol::{
    LogChunk, LogEnd, LogRequest, LogStreamKind, MessageType, ProtocolEnvelope, StructuredError,
    from_cbor,
};
use serde::Deserialize;
use tokio::time;
use uuid::Uuid;
use web_transport_quinn::Session;

use crate::{
    cli::LogsArgs,
    commands::{CommandResult, build, deploy},
};

const NON_FOLLOW_IDLE_TIMEOUT_SECS: u64 = 2;

#[derive(Debug, Deserialize)]
struct LogsRequestAck {
    accepted: bool,
    process_ids: Vec<String>,
    #[allow(dead_code)]
    follow: bool,
}

#[derive(Debug, Deserialize)]
struct LogsDatagramHeader {
    #[serde(rename = "type")]
    message_type: MessageType,
    request_id: Uuid,
    #[serde(default)]
    error: Option<StructuredError>,
}

#[derive(Debug)]
enum LogsDatagram {
    Chunk(LogChunk),
    End(LogEnd),
}

pub fn run(args: LogsArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(args: LogsArgs, project_root: &Path) -> CommandResult {
    match run_inner(args, project_root) {
        Ok(()) => CommandResult {
            exit_code: 0,
            stderr: None,
        },
        Err(err) => CommandResult {
            exit_code: 2,
            stderr: Some(err.to_string()),
        },
    }
}

fn run_inner(args: LogsArgs, project_root: &Path) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?;
    runtime.block_on(run_async(args, project_root))
}

async fn run_async(args: LogsArgs, project_root: &Path) -> anyhow::Result<()> {
    let target = build::load_target_config(None, build::default_target_name(), project_root)
        .context("failed to load target configuration")?
        .require_deploy_credentials()
        .context("target settings are invalid for logs")?;
    let session = deploy::connect_target(&target).await?;

    let request_id = Uuid::new_v4();
    let correlation_id = Uuid::new_v4();
    let request = deploy::request_envelope(
        MessageType::LogsRequest,
        request_id,
        correlation_id,
        &LogRequest {
            process_id: args.process_id.clone(),
            follow: args.follow,
            tail_lines: args.tail,
        },
    )?;
    let ack: LogsRequestAck =
        deploy::response_payload(deploy::request_response(&session, &request).await?)?;
    if !ack.accepted {
        return Err(anyhow!("logs.request was not accepted"));
    }
    if ack.process_ids.is_empty() {
        return Err(anyhow!("logs.request returned no target process"));
    }

    receive_logs_datagrams(&session, request_id, args.follow, args.process_id.is_none()).await?;
    Ok(())
}

async fn receive_logs_datagrams(
    session: &Session,
    request_id: Uuid,
    follow: bool,
    all_processes: bool,
) -> anyhow::Result<()> {
    let mut expected_seq: Option<u64> = None;
    let mut truncated_warned = false;

    loop {
        let datagram = if follow {
            tokio::select! {
                result = session.read_datagram() => Some(result.context("failed to read log datagram")?),
                _ = tokio::signal::ctrl_c() => None,
            }
        } else {
            match time::timeout(
                Duration::from_secs(NON_FOLLOW_IDLE_TIMEOUT_SECS),
                session.read_datagram(),
            )
            .await
            {
                Ok(result) => Some(result.context("failed to read log datagram")?),
                Err(_) => None,
            }
        };

        let Some(datagram) = datagram else {
            break;
        };

        let Some(message) = decode_logs_datagram(&datagram, request_id)? else {
            continue;
        };
        match message {
            LogsDatagram::Chunk(chunk) => {
                if request_id != chunk.request_id {
                    continue;
                }
                if detect_seq_gap(&mut expected_seq, chunk.seq) && !truncated_warned {
                    eprintln!("<<logs truncated>>");
                    truncated_warned = true;
                }
                render_chunk(&chunk, all_processes);
                if !follow && chunk.is_last {
                    break;
                }
            }
            LogsDatagram::End(end) => {
                if request_id != end.request_id {
                    continue;
                }
                if detect_seq_gap(&mut expected_seq, end.seq) && !truncated_warned {
                    eprintln!("<<logs truncated>>");
                }
                if let Some(error) = end.error {
                    return Err(anyhow!(
                        "logs stream ended with error: {} ({:?})",
                        error.message,
                        error.code
                    ));
                }
                break;
            }
        }
    }

    Ok(())
}

fn decode_logs_datagram(datagram: &[u8], request_id: Uuid) -> anyhow::Result<Option<LogsDatagram>> {
    let header: LogsDatagramHeader =
        from_cbor(datagram).context("failed to decode log datagram header")?;
    if let Some(err) = header.error {
        return Err(anyhow!(
            "server error: {} ({:?}) at {}",
            err.message,
            err.code,
            err.stage
        ));
    }
    if header.request_id != request_id {
        return Ok(None);
    }

    match header.message_type {
        MessageType::LogsChunk => {
            let envelope: ProtocolEnvelope<LogChunk> =
                from_cbor(datagram).context("failed to decode logs.chunk datagram")?;
            if envelope.payload.request_id != request_id {
                return Ok(None);
            }
            Ok(Some(LogsDatagram::Chunk(envelope.payload)))
        }
        MessageType::LogsEnd => {
            let envelope: ProtocolEnvelope<LogEnd> =
                from_cbor(datagram).context("failed to decode logs.end datagram")?;
            if envelope.payload.request_id != request_id {
                return Ok(None);
            }
            Ok(Some(LogsDatagram::End(envelope.payload)))
        }
        _ => Ok(None),
    }
}

fn detect_seq_gap(expected_seq: &mut Option<u64>, actual: u64) -> bool {
    let gap = expected_seq.is_some_and(|expected| actual != expected);
    *expected_seq = Some(actual.saturating_add(1));
    gap
}

fn render_chunk(chunk: &LogChunk, all_processes: bool) {
    if chunk.bytes.is_empty() {
        return;
    }

    if all_processes {
        let rendered = format_prefixed_lines(&chunk.process_id, chunk.stream_kind, &chunk.bytes);
        print!("{rendered}");
        return;
    }

    let text = String::from_utf8_lossy(&chunk.bytes);
    match chunk.stream_kind {
        LogStreamKind::Stderr => eprint!("{text}"),
        LogStreamKind::Stdout | LogStreamKind::Composite => print!("{text}"),
    }
}

fn format_prefixed_lines(process_id: &str, stream_kind: LogStreamKind, bytes: &[u8]) -> String {
    let prefix = format!("[{}][{}] ", process_id, stream_kind_label(stream_kind));
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    for segment in text.split_inclusive('\n') {
        out.push_str(&prefix);
        out.push_str(segment);
        if !segment.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn stream_kind_label(stream_kind: LogStreamKind) -> &'static str {
    match stream_kind {
        LogStreamKind::Stdout => "stdout",
        LogStreamKind::Stderr => "stderr",
        LogStreamKind::Composite => "composite",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imago_protocol::to_cbor;

    #[test]
    fn detect_seq_gap_reports_mismatch() {
        let mut expected = None;
        assert!(!detect_seq_gap(&mut expected, 0));
        assert!(!detect_seq_gap(&mut expected, 1));
        assert!(detect_seq_gap(&mut expected, 3));
        assert!(!detect_seq_gap(&mut expected, 4));
    }

    #[test]
    fn format_prefixed_lines_adds_prefix_for_each_line() {
        let rendered = format_prefixed_lines("svc-a", LogStreamKind::Stdout, b"a\nb");
        assert_eq!(rendered, "[svc-a][stdout] a\n[svc-a][stdout] b\n");
    }

    #[test]
    fn decode_logs_datagram_decodes_typed_chunk_payload() {
        let request_id = Uuid::new_v4();
        let envelope = ProtocolEnvelope::new(
            MessageType::LogsChunk,
            request_id,
            Uuid::new_v4(),
            LogChunk {
                request_id,
                seq: 3,
                process_id: "svc-a".to_string(),
                stream_kind: LogStreamKind::Stdout,
                bytes: b"hello".to_vec(),
                is_last: false,
            },
        );
        let datagram = to_cbor(&envelope).expect("encoding should succeed");

        let decoded = decode_logs_datagram(&datagram, request_id).expect("decode should succeed");
        match decoded {
            Some(LogsDatagram::Chunk(chunk)) => {
                assert_eq!(chunk.seq, 3);
                assert_eq!(chunk.bytes, b"hello".to_vec());
            }
            _ => panic!("expected chunk datagram"),
        }
    }
}
