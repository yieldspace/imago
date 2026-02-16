use std::{
    borrow::Cow,
    io::{self, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow};
use imago_protocol::{
    LogChunk, LogEnd, LogRequest, LogStreamKind, MessageType, ProtocolEnvelope, StructuredError,
    from_cbor,
};
use serde::{Deserialize, Serialize};
use tokio::time;
use uuid::Uuid;
use web_transport_quinn::Session;

use crate::{
    cli::LogsArgs,
    commands::{CommandResult, build, deploy},
};

const NON_FOLLOW_IDLE_TIMEOUT_SECS: u64 = 2;
const POST_END_DRAIN_TIMEOUT_MS: u64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogsOutputFormat {
    Text,
    JsonLines,
}

#[derive(Debug, Serialize)]
struct JsonLogLine<'a> {
    name: &'a str,
    stream: &'a str,
    timestamp: String,
    log: &'a str,
}

#[derive(Debug, Default)]
struct PrefixRenderState {
    streams: Vec<StreamPrefixState>,
}

#[derive(Debug)]
struct StreamPrefixState {
    name: String,
    stream_kind: LogStreamKind,
    at_line_start: bool,
}

#[derive(Debug, Default)]
struct JsonLinesRenderState {
    streams: Vec<StreamJsonState>,
}

#[derive(Debug)]
struct StreamJsonState {
    name: String,
    stream_kind: LogStreamKind,
    pending: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct BufferedJsonLogLine {
    name: String,
    stream_kind: LogStreamKind,
    log: String,
}

impl PrefixRenderState {
    fn at_line_start(&self, name: &str, stream_kind: LogStreamKind) -> bool {
        self.streams
            .iter()
            .find(|state| state.name == name && state.stream_kind == stream_kind)
            .map(|state| state.at_line_start)
            .unwrap_or(true)
    }

    fn set_at_line_start(&mut self, name: &str, stream_kind: LogStreamKind, at_line_start: bool) {
        if let Some(state) = self
            .streams
            .iter_mut()
            .find(|state| state.name == name && state.stream_kind == stream_kind)
        {
            state.at_line_start = at_line_start;
            return;
        }

        self.streams.push(StreamPrefixState {
            name: name.to_string(),
            stream_kind,
            at_line_start,
        });
    }
}

impl JsonLinesRenderState {
    fn pending_mut(&mut self, name: &str, stream_kind: LogStreamKind) -> &mut Vec<u8> {
        if let Some(index) = self
            .streams
            .iter()
            .position(|state| state.name == name && state.stream_kind == stream_kind)
        {
            return &mut self.streams[index].pending;
        }

        self.streams.push(StreamJsonState {
            name: name.to_string(),
            stream_kind,
            pending: Vec::new(),
        });
        let last_index = self.streams.len().saturating_sub(1);
        &mut self.streams[last_index].pending
    }
}

#[derive(Debug, Deserialize)]
struct LogsRequestAck {
    accepted: bool,
    names: Vec<String>,
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
            name: args.name.clone(),
            follow: args.follow,
            tail_lines: args.tail,
        },
    )?;
    let ack: LogsRequestAck =
        deploy::response_payload(deploy::request_response(&session, &request).await?)?;
    if !ack.accepted {
        return Err(anyhow!("logs.request was not accepted"));
    }
    if ack.names.is_empty() {
        return Err(anyhow!("logs.request returned no target service"));
    }

    let output_format = if args.json {
        LogsOutputFormat::JsonLines
    } else {
        LogsOutputFormat::Text
    };
    receive_logs_datagrams(
        &session,
        request_id,
        args.follow,
        args.name.is_none(),
        output_format,
    )
    .await?;
    Ok(())
}

async fn receive_logs_datagrams(
    session: &Session,
    request_id: Uuid,
    follow: bool,
    all_processes: bool,
    output_format: LogsOutputFormat,
) -> anyhow::Result<()> {
    let mut expected_seq: Option<u64> = None;
    let mut truncated_warned = false;
    let mut prefix_state = PrefixRenderState::default();
    let mut json_state = JsonLinesRenderState::default();

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
                Err(_) => {
                    return Err(anyhow!(
                        "timed out waiting for logs.end after {}s",
                        NON_FOLLOW_IDLE_TIMEOUT_SECS
                    ));
                }
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
                warn_if_seq_gap(&mut expected_seq, chunk.seq, &mut truncated_warned);
                render_chunk(
                    &chunk,
                    all_processes,
                    output_format,
                    &mut prefix_state,
                    &mut json_state,
                )?;
            }
            LogsDatagram::End(end) => {
                if request_id != end.request_id {
                    continue;
                }
                if let Some(error) = end.error {
                    return Err(anyhow!(
                        "logs stream ended with error: {} ({:?})",
                        error.message,
                        error.code
                    ));
                }
                let delayed_chunk_seqs = drain_post_end_chunks(
                    session,
                    request_id,
                    all_processes,
                    output_format,
                    &mut prefix_state,
                    &mut json_state,
                    end.seq,
                )
                .await?;
                apply_end_seq_after_drain(
                    &mut expected_seq,
                    end.seq,
                    &delayed_chunk_seqs,
                    &mut truncated_warned,
                );
                break;
            }
        }
    }

    flush_json_tail_if_needed(output_format, &mut json_state)?;
    Ok(())
}

async fn drain_post_end_chunks(
    session: &Session,
    request_id: Uuid,
    all_processes: bool,
    output_format: LogsOutputFormat,
    prefix_state: &mut PrefixRenderState,
    json_state: &mut JsonLinesRenderState,
    end_seq: u64,
) -> anyhow::Result<Vec<u64>> {
    let deadline = time::Instant::now() + Duration::from_millis(POST_END_DRAIN_TIMEOUT_MS);
    let mut delayed_chunk_seqs = Vec::new();

    loop {
        let now = time::Instant::now();
        if now >= deadline {
            break;
        }
        let wait_for = deadline.saturating_duration_since(now);
        let datagram = match time::timeout(wait_for, session.read_datagram()).await {
            Ok(result) => result.context("failed to read post-end log datagram")?,
            Err(_) => break,
        };
        let Some(message) = decode_logs_datagram(&datagram, request_id)? else {
            continue;
        };
        let LogsDatagram::Chunk(chunk) = message else {
            continue;
        };
        if chunk.seq >= end_seq {
            continue;
        }
        render_chunk(
            &chunk,
            all_processes,
            output_format,
            prefix_state,
            json_state,
        )?;
        delayed_chunk_seqs.push(chunk.seq);
    }

    Ok(delayed_chunk_seqs)
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
    let gap = match expected_seq {
        Some(expected) => actual != *expected,
        None => actual != 0,
    };
    *expected_seq = Some(actual.saturating_add(1));
    gap
}

fn warn_if_seq_gap(expected_seq: &mut Option<u64>, actual: u64, truncated_warned: &mut bool) {
    if detect_seq_gap(expected_seq, actual) && !*truncated_warned {
        eprintln!("<<logs truncated>>");
        *truncated_warned = true;
    }
}

fn apply_end_seq_after_drain(
    expected_seq: &mut Option<u64>,
    end_seq: u64,
    delayed_chunk_seqs: &[u64],
    truncated_warned: &mut bool,
) {
    for seq in delayed_chunk_seqs {
        warn_if_seq_gap(expected_seq, *seq, truncated_warned);
    }
    warn_if_seq_gap(expected_seq, end_seq, truncated_warned);
}

fn render_chunk(
    chunk: &LogChunk,
    all_processes: bool,
    output_format: LogsOutputFormat,
    prefix_state: &mut PrefixRenderState,
    json_state: &mut JsonLinesRenderState,
) -> anyhow::Result<()> {
    if chunk.bytes.is_empty() {
        return Ok(());
    }

    match output_format {
        LogsOutputFormat::Text => render_text_chunk(chunk, all_processes, prefix_state),
        LogsOutputFormat::JsonLines => render_json_chunk(chunk, json_state),
    }
}

fn flush_json_tail_if_needed(
    output_format: LogsOutputFormat,
    json_state: &mut JsonLinesRenderState,
) -> anyhow::Result<()> {
    if output_format != LogsOutputFormat::JsonLines {
        return Ok(());
    }
    let lines = flush_json_line_buffers(json_state);
    write_json_lines(&lines)
}

fn render_text_chunk(
    chunk: &LogChunk,
    all_processes: bool,
    prefix_state: &mut PrefixRenderState,
) -> anyhow::Result<()> {
    let rendered = renderable_chunk_bytes(chunk, all_processes, prefix_state);
    if all_processes
        || matches!(
            chunk.stream_kind,
            LogStreamKind::Stdout | LogStreamKind::Composite
        )
    {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(rendered.as_ref())
            .context("failed to write log chunk to stdout")?;
    } else {
        let mut stderr = io::stderr().lock();
        stderr
            .write_all(rendered.as_ref())
            .context("failed to write log chunk to stderr")?;
    }

    Ok(())
}

fn render_json_chunk(
    chunk: &LogChunk,
    json_state: &mut JsonLinesRenderState,
) -> anyhow::Result<()> {
    let lines = collect_json_lines_from_chunk(chunk, json_state);
    write_json_lines(&lines)
}

fn write_json_lines(lines: &[BufferedJsonLogLine]) -> anyhow::Result<()> {
    if lines.is_empty() {
        return Ok(());
    }

    let mut stdout = io::stdout().lock();
    for line in lines {
        let payload = JsonLogLine {
            name: &line.name,
            stream: stream_kind_label(line.stream_kind),
            timestamp: current_timestamp_unix_secs(),
            log: &line.log,
        };
        serde_json::to_writer(&mut stdout, &payload).context("failed to encode json log line")?;
        stdout
            .write_all(b"\n")
            .context("failed to write json log line delimiter")?;
    }
    Ok(())
}

fn collect_json_lines_from_chunk(
    chunk: &LogChunk,
    json_state: &mut JsonLinesRenderState,
) -> Vec<BufferedJsonLogLine> {
    let mut lines = Vec::new();
    let pending = json_state.pending_mut(&chunk.name, chunk.stream_kind);
    pending.extend_from_slice(&chunk.bytes);
    drain_complete_lines(&chunk.name, chunk.stream_kind, pending, &mut lines);
    lines
}

fn flush_json_line_buffers(json_state: &mut JsonLinesRenderState) -> Vec<BufferedJsonLogLine> {
    let mut lines = Vec::new();
    for stream in &mut json_state.streams {
        if stream.pending.is_empty() {
            continue;
        }
        lines.push(BufferedJsonLogLine {
            name: stream.name.clone(),
            stream_kind: stream.stream_kind,
            log: normalize_log_line(std::mem::take(&mut stream.pending)),
        });
    }
    lines
}

fn drain_complete_lines(
    name: &str,
    stream_kind: LogStreamKind,
    pending: &mut Vec<u8>,
    out: &mut Vec<BufferedJsonLogLine>,
) {
    let mut consumed = 0usize;
    for (idx, byte) in pending.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        out.push(BufferedJsonLogLine {
            name: name.to_string(),
            stream_kind,
            log: normalize_log_line(pending[consumed..idx].to_vec()),
        });
        consumed = idx.saturating_add(1);
    }
    if consumed > 0 {
        pending.drain(..consumed);
    }
}

fn normalize_log_line(mut bytes: Vec<u8>) -> String {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn current_timestamp_unix_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn renderable_chunk_bytes<'a>(
    chunk: &'a LogChunk,
    all_processes: bool,
    prefix_state: &mut PrefixRenderState,
) -> Cow<'a, [u8]> {
    if !all_processes {
        return Cow::Borrowed(&chunk.bytes);
    }

    let at_line_start = prefix_state.at_line_start(&chunk.name, chunk.stream_kind);
    let (rendered, next_at_line_start) =
        format_prefixed_bytes(&chunk.name, chunk.stream_kind, &chunk.bytes, at_line_start);
    prefix_state.set_at_line_start(&chunk.name, chunk.stream_kind, next_at_line_start);
    Cow::Owned(rendered)
}

fn format_prefixed_bytes(
    name: &str,
    stream_kind: LogStreamKind,
    bytes: &[u8],
    mut at_line_start: bool,
) -> (Vec<u8>, bool) {
    let prefix = format!("[{}][{}] ", name, stream_kind_label(stream_kind));
    let prefix_bytes = prefix.as_bytes();
    let mut out = Vec::with_capacity(bytes.len().saturating_add(prefix_bytes.len()));

    let mut segment_start = 0usize;
    while segment_start < bytes.len() {
        if at_line_start {
            out.extend_from_slice(prefix_bytes);
        }

        match bytes[segment_start..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            Some(offset) => {
                let segment_end = segment_start + offset + 1;
                out.extend_from_slice(&bytes[segment_start..segment_end]);
                segment_start = segment_end;
                at_line_start = true;
            }
            None => {
                out.extend_from_slice(&bytes[segment_start..]);
                segment_start = bytes.len();
                at_line_start = false;
            }
        }
    }

    (out, at_line_start)
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
    fn detect_seq_gap_flags_nonzero_first_sequence() {
        let mut expected = None;

        assert!(detect_seq_gap(&mut expected, 2));
        assert_eq!(expected, Some(3));
    }

    #[test]
    fn apply_end_seq_after_drain_accepts_delayed_chunk_before_end() {
        let mut expected = None;
        let mut truncated_warned = false;
        warn_if_seq_gap(&mut expected, 0, &mut truncated_warned);

        apply_end_seq_after_drain(&mut expected, 2, &[1], &mut truncated_warned);

        assert!(!truncated_warned);
        assert_eq!(expected, Some(3));
    }

    #[test]
    fn apply_end_seq_after_drain_marks_truncation_when_chunk_missing() {
        let mut expected = None;
        let mut truncated_warned = false;
        warn_if_seq_gap(&mut expected, 0, &mut truncated_warned);

        apply_end_seq_after_drain(&mut expected, 2, &[], &mut truncated_warned);

        assert!(truncated_warned);
        assert_eq!(expected, Some(3));
    }

    #[test]
    fn format_prefixed_bytes_adds_prefix_for_each_newline_terminated_line() {
        let (rendered, at_line_start) =
            format_prefixed_bytes("svc-a", LogStreamKind::Stdout, b"a\nb\n", true);
        assert_eq!(rendered, b"[svc-a][stdout] a\n[svc-a][stdout] b\n");
        assert!(at_line_start);
    }

    #[test]
    fn renderable_chunk_bytes_keeps_partial_line_contiguous_across_chunks() {
        let request_id = Uuid::new_v4();
        let mut prefix_state = PrefixRenderState::default();
        let first = LogChunk {
            request_id,
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"hel".to_vec(),
            is_last: false,
        };
        let second = LogChunk {
            request_id,
            seq: 1,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"lo\n".to_vec(),
            is_last: false,
        };

        assert_eq!(
            renderable_chunk_bytes(&first, true, &mut prefix_state).as_ref(),
            b"[svc-a][stdout] hel"
        );
        assert_eq!(
            renderable_chunk_bytes(&second, true, &mut prefix_state).as_ref(),
            b"lo\n"
        );
    }

    #[test]
    fn renderable_chunk_bytes_keeps_non_utf8_fragments_unchanged() {
        let request_id = Uuid::new_v4();
        let mut prefix_state = PrefixRenderState::default();
        let first = LogChunk {
            request_id,
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: vec![0xe3, 0x81],
            is_last: false,
        };
        let second = LogChunk {
            request_id,
            seq: 1,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: vec![0x82],
            is_last: false,
        };

        assert_eq!(
            renderable_chunk_bytes(&first, false, &mut prefix_state).as_ref(),
            &[0xe3, 0x81]
        );
        assert_eq!(
            renderable_chunk_bytes(&second, false, &mut prefix_state).as_ref(),
            &[0x82]
        );
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
                name: "svc-a".to_string(),
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

    #[test]
    fn json_lines_are_built_per_line_with_cross_chunk_join() {
        let request_id = Uuid::new_v4();
        let mut state = JsonLinesRenderState::default();
        let first = LogChunk {
            request_id,
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"hel".to_vec(),
            is_last: false,
        };
        let second = LogChunk {
            request_id,
            seq: 1,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"lo\nnext\ntail".to_vec(),
            is_last: false,
        };

        assert_eq!(collect_json_lines_from_chunk(&first, &mut state), vec![]);
        assert_eq!(
            collect_json_lines_from_chunk(&second, &mut state),
            vec![
                BufferedJsonLogLine {
                    name: "svc-a".to_string(),
                    stream_kind: LogStreamKind::Stdout,
                    log: "hello".to_string(),
                },
                BufferedJsonLogLine {
                    name: "svc-a".to_string(),
                    stream_kind: LogStreamKind::Stdout,
                    log: "next".to_string(),
                },
            ]
        );
        assert_eq!(
            flush_json_line_buffers(&mut state),
            vec![BufferedJsonLogLine {
                name: "svc-a".to_string(),
                stream_kind: LogStreamKind::Stdout,
                log: "tail".to_string(),
            }]
        );
    }

    #[test]
    fn json_lines_keep_stream_buffers_isolated() {
        let request_id = Uuid::new_v4();
        let mut state = JsonLinesRenderState::default();
        let stdout_partial = LogChunk {
            request_id,
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"abc".to_vec(),
            is_last: false,
        };
        let stderr_with_newline = LogChunk {
            request_id,
            seq: 1,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stderr,
            bytes: b"err\n".to_vec(),
            is_last: false,
        };
        let stdout_finish = LogChunk {
            request_id,
            seq: 2,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"def\n".to_vec(),
            is_last: false,
        };

        assert_eq!(
            collect_json_lines_from_chunk(&stdout_partial, &mut state),
            vec![]
        );
        assert_eq!(
            collect_json_lines_from_chunk(&stderr_with_newline, &mut state),
            vec![BufferedJsonLogLine {
                name: "svc-a".to_string(),
                stream_kind: LogStreamKind::Stderr,
                log: "err".to_string(),
            }]
        );
        assert_eq!(
            collect_json_lines_from_chunk(&stdout_finish, &mut state),
            vec![BufferedJsonLogLine {
                name: "svc-a".to_string(),
                stream_kind: LogStreamKind::Stdout,
                log: "abcdef".to_string(),
            }]
        );
    }

    #[test]
    fn json_log_line_serializes_stream_field() {
        let line = JsonLogLine {
            name: "svc-a",
            stream: "stderr",
            timestamp: "123".to_string(),
            log: "oops",
        };
        let value = serde_json::to_value(line).expect("json serialization should succeed");

        assert_eq!(value["name"], "svc-a");
        assert_eq!(value["stream"], "stderr");
        assert_eq!(value["timestamp"], "123");
        assert_eq!(value["log"], "oops");
    }

    #[test]
    fn normalize_log_line_trims_trailing_carriage_return() {
        assert_eq!(normalize_log_line(b"hello\r".to_vec()), "hello");
    }
}
