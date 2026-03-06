use std::{
    borrow::Cow,
    io::{self, Write},
    path::Path,
    time::Duration,
    time::Instant,
};

use anyhow::{Context, anyhow};
use chrono::{DateTime, Local, Utc};
use imago_protocol::{
    LogChunk, LogEnd, LogRequest, LogStreamKind, MessageType, ProtocolEnvelope, StructuredError,
    from_cbor,
};
use serde::Deserialize;
use tokio::time;
use uuid::Uuid;

use crate::{
    cli::LogsArgs,
    commands::{
        CommandResult, build,
        command_common::{
            format_local_context_line, format_peer_context_line, negotiate_hello_with_features,
        },
        deploy,
        error_diagnostics::{format_command_error, summarize_command_failure},
        ui,
    },
};

const NON_FOLLOW_IDLE_TIMEOUT_SECS: u64 = 2;
const POST_END_DRAIN_TIMEOUT_MS: u64 = 200;
const LOGS_HELLO_REQUIRED_FEATURES: [&str; 1] = ["logs.request"];
const LOGS_HELLO_REQUIRED_FEATURES_WITH_TIMESTAMP: [&str; 2] =
    ["logs.request", "logs.chunk.timestamp"];
const LOGS_STREAM_FEATURE: &str = "logs.stream";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogsTermination {
    Completed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LogsSummary {
    name: String,
    target_name: String,
    follow: bool,
    tail: u32,
    with_timestamp: bool,
    termination: LogsTermination,
}

fn logs_service_for_context(name: Option<&str>) -> &str {
    name.unwrap_or("<all-running>")
}

fn max_name_width_from_ack_names(names: &[String]) -> usize {
    names
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Default)]
struct PrefixRenderState {
    streams: Vec<StreamPrefixState>,
    max_name_width_chars: usize,
}

#[derive(Debug)]
struct StreamPrefixState {
    name: String,
    stream_kind: LogStreamKind,
    at_line_start: bool,
}

impl PrefixRenderState {
    fn with_initial_name_width(initial_name_width_chars: usize) -> Self {
        Self {
            streams: Vec::new(),
            max_name_width_chars: initial_name_width_chars,
        }
    }

    fn observe_name(&mut self, name: &str) {
        let observed = name.chars().count();
        if observed > self.max_name_width_chars {
            self.max_name_width_chars = observed;
        }
    }

    fn current_name_width(&self) -> usize {
        self.max_name_width_chars.max(1)
    }

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

pub async fn run(args: LogsArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(args: LogsArgs, project_root: &Path) -> CommandResult {
    run_with_project_root_and_target_override(args, project_root, None).await
}

pub(crate) async fn run_with_project_root_and_target_override(
    args: LogsArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("service.logs", "starting");
    match run_async_with_target_override(args, project_root, target_override).await {
        Ok(summary) => build_logs_success_result(summary, started_at),
        Err(err) => {
            let summary_message = summarize_command_failure("service.logs", &err);
            let diagnostic_message = format_logs_error_message(&err);
            ui::command_finish("service.logs", false, &summary_message);
            CommandResult::failure("service.logs", started_at, diagnostic_message)
        }
    }
}

fn format_logs_error_message(err: &anyhow::Error) -> String {
    format_command_error("service.logs", err)
}

fn build_logs_success_result(summary: LogsSummary, started_at: Instant) -> CommandResult {
    let mut result = CommandResult::success("service.logs", started_at);
    result.meta.insert("name".to_string(), summary.name);
    result
        .meta
        .insert("target".to_string(), summary.target_name);
    result
        .meta
        .insert("follow".to_string(), summary.follow.to_string());
    result
        .meta
        .insert("tail".to_string(), summary.tail.to_string());
    result.meta.insert(
        "with_timestamp".to_string(),
        summary.with_timestamp.to_string(),
    );
    result.meta.insert(
        "termination".to_string(),
        match summary.termination {
            LogsTermination::Completed => "completed",
            LogsTermination::Interrupted => "interrupted",
        }
        .to_string(),
    );
    result.meta.insert(
        "_suppress_success_meta_output".to_string(),
        "true".to_string(),
    );
    result
}

async fn run_async_with_target_override(
    args: LogsArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
) -> anyhow::Result<LogsSummary> {
    let LogsArgs {
        name,
        follow,
        tail,
        with_timestamp,
    } = args;
    let target_name = if target_override.is_some() {
        "override".to_string()
    } else {
        build::default_target_name().to_string()
    };
    let service_name = logs_service_for_context(name.as_deref());
    ui::command_stage(
        "service.logs",
        "load-config",
        "loading target configuration",
    );
    let target = match target_override {
        Some(target) => target.clone(),
        None => build::load_target_config(&target_name, project_root)
            .context("failed to load target configuration")?,
    }
    .require_deploy_credentials()
    .context("target settings are invalid for service logs")?;
    ui::command_info(
        "service.logs",
        &format_local_context_line(
            project_root,
            service_name,
            &target_name,
            &target.remote,
            target.server_name.as_deref(),
        ),
    );
    ui::command_stage("service.logs", "connect", "connecting target");
    let connected = deploy::connect_target(&target).await?;
    ui::command_stage("service.logs", "hello", "negotiating hello");
    let required_features = if with_timestamp {
        LOGS_HELLO_REQUIRED_FEATURES_WITH_TIMESTAMP.as_slice()
    } else {
        LOGS_HELLO_REQUIRED_FEATURES.as_slice()
    };
    let hello =
        negotiate_hello_with_features(&connected, Uuid::new_v4(), required_features).await?;
    ui::command_info(
        "service.logs",
        &format_peer_context_line(&connected.authority, &connected.resolved_addr, &hello),
    );

    let request_id = Uuid::new_v4();
    let correlation_id = Uuid::new_v4();
    let request = deploy::request_envelope(
        MessageType::LogsRequest,
        request_id,
        correlation_id,
        &LogRequest {
            name: name.clone(),
            follow,
            tail_lines: tail,
            with_timestamp,
        },
    )?;
    let use_stream_logs = hello
        .features
        .iter()
        .any(|feature| feature == LOGS_STREAM_FEATURE);
    let termination = if use_stream_logs {
        let mut ack: Option<LogsRequestAck> = None;
        let mut saw_end = false;
        let mut expected_seq: Option<u64> = None;
        let mut truncated_warned = false;
        let mut prefix_state = PrefixRenderState::default();
        let stream_termination = deploy::request_streamed_events(
            &connected,
            &request,
            deploy::resolve_deploy_stream_timeout(),
            (!follow).then_some(Duration::from_secs(NON_FOLLOW_IDLE_TIMEOUT_SECS)),
            follow,
            |envelope| match envelope.message_type {
                MessageType::LogsRequest => {
                    let response: LogsRequestAck = deploy::response_payload(envelope)?;
                    if !response.accepted {
                        return Err(anyhow!("logs.request was not accepted"));
                    }
                    if response.names.is_empty() {
                        return Err(anyhow!("logs.request returned no target service"));
                    }
                    prefix_state = PrefixRenderState::with_initial_name_width(
                        max_name_width_from_ack_names(&response.names),
                    );
                    ack = Some(response);
                    ui::command_clear("service.logs");
                    Ok(false)
                }
                MessageType::LogsChunk => {
                    if ack.is_none() {
                        return Err(anyhow!("logs.chunk arrived before logs.request ack"));
                    }
                    let chunk: LogChunk = deploy::response_payload(envelope)?;
                    if request_id != chunk.request_id {
                        return Ok(false);
                    }
                    warn_if_seq_gap(&mut expected_seq, chunk.seq, &mut truncated_warned);
                    render_chunk(&chunk, name.is_none(), with_timestamp, &mut prefix_state)?;
                    Ok(false)
                }
                MessageType::LogsEnd => {
                    if ack.is_none() {
                        return Err(anyhow!("logs.end arrived before logs.request ack"));
                    }
                    let end: LogEnd = deploy::response_payload(envelope)?;
                    if request_id != end.request_id {
                        return Ok(false);
                    }
                    if let Some(error) = end.error {
                        return Err(anyhow!(
                            "logs stream ended with error: {} ({:?})",
                            error.message,
                            error.code
                        ));
                    }
                    apply_end_seq_after_drain(
                        &mut expected_seq,
                        end.seq,
                        &[],
                        &mut truncated_warned,
                    );
                    saw_end = true;
                    Ok(true)
                }
                _ => Ok(false),
            },
        )
        .await?;
        if ack.is_none() {
            return Err(anyhow!("logs.request returned empty response stream"));
        }
        if stream_termination == deploy::StreamRequestTermination::Completed && !saw_end {
            return Err(anyhow!("logs stream ended without logs.end"));
        }
        match stream_termination {
            deploy::StreamRequestTermination::Completed => LogsTermination::Completed,
            deploy::StreamRequestTermination::Interrupted => LogsTermination::Interrupted,
        }
    } else {
        if connected.uses_ssh_transport() {
            return Err(anyhow!(
                "ssh target requires server support for logs.stream"
            ));
        }
        let ack: LogsRequestAck =
            deploy::response_payload(deploy::request_response(&connected, &request).await?)?;
        if !ack.accepted {
            return Err(anyhow!("logs.request was not accepted"));
        }
        if ack.names.is_empty() {
            return Err(anyhow!("logs.request returned no target service"));
        }
        ui::command_clear("service.logs");

        let initial_name_width_chars = max_name_width_from_ack_names(&ack.names);
        receive_logs_datagrams(
            connected
                .as_quinn_session()
                .ok_or_else(|| anyhow!("logs datagram fallback requires direct target"))?,
            request_id,
            follow,
            name.is_none(),
            with_timestamp,
            initial_name_width_chars,
        )
        .await?
    };
    Ok(LogsSummary {
        name: name.unwrap_or_else(|| "<all-running>".to_string()),
        target_name,
        follow,
        tail,
        with_timestamp,
        termination,
    })
}

async fn receive_logs_datagrams(
    session: &web_transport_quinn::Session,
    request_id: Uuid,
    follow: bool,
    all_processes: bool,
    with_timestamp: bool,
    initial_name_width_chars: usize,
) -> anyhow::Result<LogsTermination> {
    let mut expected_seq: Option<u64> = None;
    let mut truncated_warned = false;
    let mut prefix_state = PrefixRenderState::with_initial_name_width(initial_name_width_chars);

    'stream: loop {
        let datagram_result = if follow {
            tokio::select! {
                result = session.read_datagram() => Some(result.context("failed to read log datagram")),
                _ = tokio::signal::ctrl_c() => None,
            }
        } else {
            match time::timeout(
                Duration::from_secs(NON_FOLLOW_IDLE_TIMEOUT_SECS),
                session.read_datagram(),
            )
            .await
            {
                Ok(result) => Some(result.context("failed to read log datagram")),
                Err(_) => {
                    break 'stream Err(anyhow!(
                        "timed out waiting for logs.end after {}s",
                        NON_FOLLOW_IDLE_TIMEOUT_SECS
                    ));
                }
            }
        };

        let Some(datagram_result) = datagram_result else {
            break 'stream Ok(LogsTermination::Interrupted);
        };
        let datagram = match datagram_result {
            Ok(datagram) => datagram,
            Err(err) => break 'stream Err(err),
        };

        let message = match decode_logs_datagram(&datagram, request_id) {
            Ok(Some(message)) => message,
            Ok(None) => continue,
            Err(err) => break 'stream Err(err),
        };
        match message {
            LogsDatagram::Chunk(chunk) => {
                if request_id != chunk.request_id {
                    continue;
                }
                warn_if_seq_gap(&mut expected_seq, chunk.seq, &mut truncated_warned);
                if let Err(err) =
                    render_chunk(&chunk, all_processes, with_timestamp, &mut prefix_state)
                {
                    break 'stream Err(err);
                }
            }
            LogsDatagram::End(end) => {
                if request_id != end.request_id {
                    continue;
                }
                if let Some(error) = end.error {
                    break 'stream Err(anyhow!(
                        "logs stream ended with error: {} ({:?})",
                        error.message,
                        error.code
                    ));
                }
                let delayed_chunk_seqs = match drain_post_end_chunks(
                    session,
                    request_id,
                    all_processes,
                    with_timestamp,
                    &mut prefix_state,
                    end.seq,
                )
                .await
                {
                    Ok(seqs) => seqs,
                    Err(err) => break 'stream Err(err),
                };
                apply_end_seq_after_drain(
                    &mut expected_seq,
                    end.seq,
                    &delayed_chunk_seqs,
                    &mut truncated_warned,
                );
                break 'stream Ok(LogsTermination::Completed);
            }
        }
    }
}

async fn drain_post_end_chunks(
    session: &web_transport_quinn::Session,
    request_id: Uuid,
    all_processes: bool,
    with_timestamp: bool,
    prefix_state: &mut PrefixRenderState,
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
        render_chunk(&chunk, all_processes, with_timestamp, prefix_state)?;
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
        ui::command_warn("service.logs", "<<logs truncated>>");
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
    with_timestamp: bool,
    prefix_state: &mut PrefixRenderState,
) -> anyhow::Result<()> {
    if chunk.bytes.is_empty() {
        return Ok(());
    }

    render_text_chunk(chunk, all_processes, with_timestamp, prefix_state)
}

fn render_text_chunk(
    chunk: &LogChunk,
    all_processes: bool,
    with_timestamp: bool,
    prefix_state: &mut PrefixRenderState,
) -> anyhow::Result<()> {
    let timestamp = format_chunk_timestamp(chunk, with_timestamp)?;
    let rendered = renderable_chunk_bytes(chunk, all_processes, timestamp.as_deref(), prefix_state);
    if should_write_text_chunk_to_stderr(chunk, all_processes) {
        let mut stderr = io::stderr().lock();
        stderr
            .write_all(rendered.as_ref())
            .context("failed to write log chunk to stderr")?;
    } else {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(rendered.as_ref())
            .context("failed to write log chunk to stdout")?;
    }

    Ok(())
}

fn should_write_text_chunk_to_stderr(chunk: &LogChunk, all_processes: bool) -> bool {
    !all_processes && matches!(chunk.stream_kind, LogStreamKind::Stderr)
}

fn renderable_chunk_bytes<'a>(
    chunk: &'a LogChunk,
    all_processes: bool,
    timestamp: Option<&str>,
    prefix_state: &mut PrefixRenderState,
) -> Cow<'a, [u8]> {
    let _ = all_processes;
    prefix_state.observe_name(&chunk.name);
    let at_line_start = prefix_state.at_line_start(&chunk.name, chunk.stream_kind);
    let (rendered, next_at_line_start) = format_structured_bytes(
        &chunk.name,
        &chunk.bytes,
        timestamp,
        at_line_start,
        prefix_state.current_name_width(),
    );
    prefix_state.set_at_line_start(&chunk.name, chunk.stream_kind, next_at_line_start);
    Cow::Owned(rendered)
}

fn format_structured_bytes(
    name: &str,
    bytes: &[u8],
    timestamp: Option<&str>,
    mut at_line_start: bool,
    name_width_chars: usize,
) -> (Vec<u8>, bool) {
    let mut out = Vec::new();

    let mut segment_start = 0usize;
    while segment_start < bytes.len() {
        if at_line_start {
            let prefix = match timestamp {
                Some(timestamp) => format!("{name:<name_width_chars$} | {timestamp} "),
                None => format!("{name:<name_width_chars$} | "),
            };
            let prefix_bytes = prefix.as_bytes();
            out.reserve(bytes.len().saturating_add(prefix_bytes.len()));
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

fn format_chunk_timestamp(
    chunk: &LogChunk,
    with_timestamp: bool,
) -> anyhow::Result<Option<String>> {
    if !with_timestamp {
        return Ok(None);
    }

    let timestamp_unix_ms = chunk
        .timestamp_unix_ms
        .ok_or_else(|| anyhow!("logs.chunk is missing timestamp_unix_ms"))?;
    format_timestamp_rfc3339_local(timestamp_unix_ms).map(Some)
}

fn format_timestamp_rfc3339_local(timestamp_unix_ms: u64) -> anyhow::Result<String> {
    let millis = i64::try_from(timestamp_unix_ms).context("timestamp_unix_ms is out of range")?;
    let utc = DateTime::<Utc>::from_timestamp_millis(millis)
        .ok_or_else(|| anyhow!("timestamp_unix_ms is invalid"))?;
    Ok(utc.with_timezone(&Local).to_rfc3339())
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
    fn format_structured_bytes_adds_prefix_for_each_newline_terminated_line() {
        let (rendered, at_line_start) = format_structured_bytes("svc-a", b"a\nb\n", None, true, 5);
        let rendered_text = String::from_utf8_lossy(&rendered);
        assert!(rendered_text.contains("svc-a | a\n"));
        assert!(rendered_text.contains("svc-a | b\n"));
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
            timestamp_unix_ms: None,
        };
        let second = LogChunk {
            request_id,
            seq: 1,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"lo\n".to_vec(),
            is_last: false,
            timestamp_unix_ms: None,
        };

        let first_rendered =
            renderable_chunk_bytes(&first, true, None, &mut prefix_state).into_owned();
        let first_text = String::from_utf8_lossy(&first_rendered);
        assert!(first_text.contains("svc-a | hel"));
        assert_eq!(
            renderable_chunk_bytes(&second, true, None, &mut prefix_state).as_ref(),
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
            timestamp_unix_ms: None,
        };
        let second = LogChunk {
            request_id,
            seq: 1,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: vec![0x82],
            is_last: false,
            timestamp_unix_ms: None,
        };

        let first_rendered =
            renderable_chunk_bytes(&first, false, None, &mut prefix_state).into_owned();
        let first_prefix_text = String::from_utf8_lossy(&first_rendered);
        assert!(first_prefix_text.contains("svc-a | "));
        assert!(first_rendered.ends_with(&[0xe3, 0x81]));
        assert_eq!(
            renderable_chunk_bytes(&second, false, None, &mut prefix_state).as_ref(),
            &[0x82]
        );
    }

    #[test]
    fn format_structured_bytes_aligns_pipe_with_given_width() {
        let (short, _) = format_structured_bytes("a", b"x\n", None, true, 8);
        let (long, _) = format_structured_bytes("longname", b"y\n", None, true, 8);
        let short_pipe = short
            .iter()
            .position(|byte| *byte == b'|')
            .expect("short output should contain pipe");
        let long_pipe = long
            .iter()
            .position(|byte| *byte == b'|')
            .expect("long output should contain pipe");

        assert_eq!(short_pipe, long_pipe);
    }

    #[test]
    fn renderable_chunk_bytes_expands_alignment_width_for_new_longer_name() {
        let request_id = Uuid::new_v4();
        let mut prefix_state = PrefixRenderState::with_initial_name_width(3);
        let short_first = LogChunk {
            request_id,
            seq: 0,
            name: "api".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"one\n".to_vec(),
            is_last: false,
            timestamp_unix_ms: None,
        };
        let longer = LogChunk {
            request_id,
            seq: 1,
            name: "longer-name".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"two\n".to_vec(),
            is_last: false,
            timestamp_unix_ms: None,
        };
        let short_after = LogChunk {
            request_id,
            seq: 2,
            name: "api".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"three\n".to_vec(),
            is_last: false,
            timestamp_unix_ms: None,
        };

        let first_rendered =
            renderable_chunk_bytes(&short_first, false, None, &mut prefix_state).into_owned();
        let longer_rendered =
            renderable_chunk_bytes(&longer, false, None, &mut prefix_state).into_owned();
        let after_rendered =
            renderable_chunk_bytes(&short_after, false, None, &mut prefix_state).into_owned();

        let first_pipe = first_rendered
            .iter()
            .position(|byte| *byte == b'|')
            .expect("first output should contain pipe");
        let longer_pipe = longer_rendered
            .iter()
            .position(|byte| *byte == b'|')
            .expect("longer output should contain pipe");
        let after_pipe = after_rendered
            .iter()
            .position(|byte| *byte == b'|')
            .expect("after output should contain pipe");

        assert!(longer_pipe > first_pipe);
        assert_eq!(after_pipe, longer_pipe);
    }

    #[test]
    fn max_name_width_from_ack_names_uses_longest_name() {
        let width = max_name_width_from_ack_names(&[
            "api".to_string(),
            "service-long-name".to_string(),
            "db".to_string(),
        ]);
        assert_eq!(width, "service-long-name".chars().count());
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
                timestamp_unix_ms: None,
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
    fn stderr_text_chunk_uses_stderr_only_for_single_process_output() {
        let chunk = LogChunk {
            request_id: Uuid::new_v4(),
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stderr,
            bytes: b"oops".to_vec(),
            is_last: false,
            timestamp_unix_ms: None,
        };
        assert!(should_write_text_chunk_to_stderr(&chunk, false));
        assert!(!should_write_text_chunk_to_stderr(&chunk, true));
    }

    #[test]
    fn stdout_text_chunk_does_not_use_stderr() {
        let chunk = LogChunk {
            request_id: Uuid::new_v4(),
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"ok".to_vec(),
            is_last: false,
            timestamp_unix_ms: None,
        };
        assert!(!should_write_text_chunk_to_stderr(&chunk, false));
        assert!(!should_write_text_chunk_to_stderr(&chunk, true));
    }

    #[test]
    fn logs_hello_required_features_are_fixed() {
        assert_eq!(LOGS_HELLO_REQUIRED_FEATURES, ["logs.request"]);
        assert_eq!(
            LOGS_HELLO_REQUIRED_FEATURES_WITH_TIMESTAMP,
            ["logs.request", "logs.chunk.timestamp"]
        );
    }

    #[test]
    fn logs_service_for_context_uses_all_running_placeholder() {
        assert_eq!(logs_service_for_context(None), "<all-running>");
        assert_eq!(logs_service_for_context(Some("svc-a")), "svc-a");
    }

    #[test]
    fn logs_error_message_uses_diagnostics_sections() {
        let err = anyhow!("failed to load target configuration");
        let message = format_logs_error_message(&err);
        assert!(message.contains("caused by:"));
        assert!(message.contains("hint:"));
        assert!(message.contains("target settings"));
    }

    #[test]
    fn logs_success_result_always_suppresses_finalize_output() {
        let result = build_logs_success_result(
            LogsSummary {
                name: "<all-running>".to_string(),
                target_name: "default".to_string(),
                follow: false,
                tail: 200,
                with_timestamp: false,
                termination: LogsTermination::Completed,
            },
            Instant::now(),
        );

        assert_eq!(
            result
                .meta
                .get("_suppress_success_meta_output")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn format_structured_bytes_includes_timestamp_prefix_when_present() {
        let (rendered, at_line_start) = format_structured_bytes(
            "svc-a",
            b"hello\n",
            Some("2026-02-26T12:34:56+09:00"),
            true,
            5,
        );
        let rendered_text = String::from_utf8_lossy(&rendered);
        assert!(rendered_text.starts_with("svc-a | 2026-02-26T12:34:56+09:00 hello\n"));
        assert!(at_line_start);
    }

    #[test]
    fn format_chunk_timestamp_requires_timestamp_when_enabled() {
        let chunk = LogChunk {
            request_id: Uuid::new_v4(),
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"ok".to_vec(),
            is_last: false,
            timestamp_unix_ms: None,
        };
        let err = format_chunk_timestamp(&chunk, true).expect_err("missing timestamp should fail");
        assert!(
            err.to_string().contains("missing timestamp_unix_ms"),
            "error should mention missing timestamp field"
        );
    }

    #[test]
    fn format_timestamp_rfc3339_local_emits_parseable_rfc3339() {
        let rendered =
            format_timestamp_rfc3339_local(1_739_700_000_123).expect("format should succeed");
        let parsed =
            chrono::DateTime::parse_from_rfc3339(&rendered).expect("timestamp should be RFC3339");
        assert_eq!(parsed.timestamp_millis(), 1_739_700_000_123);
    }
}
