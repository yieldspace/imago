use std::{
    borrow::Cow,
    io::{self, Write},
    path::Path,
    time::Duration,
    time::Instant,
};

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
    commands::{
        CommandResult, build,
        command_common::{
            format_local_context_line, format_peer_context_line, negotiate_hello_with_features,
        },
        deploy,
        error_diagnostics::format_command_error,
        ui,
    },
};

const NON_FOLLOW_IDLE_TIMEOUT_SECS: u64 = 2;
const POST_END_DRAIN_TIMEOUT_MS: u64 = 200;
const LOGS_HELLO_REQUIRED_FEATURES: [&str; 1] = ["logs.request"];

fn logs_service_for_context(name: Option<&str>) -> &str {
    name.unwrap_or("<all-running>")
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
    ui::command_start("logs", "starting");
    match run_async_with_target_override(args, project_root, target_override).await {
        Ok(()) => {
            ui::command_finish("logs", true, "completed");
            CommandResult::success("logs", started_at)
        }
        Err(err) => {
            let summary_message = err.to_string();
            let diagnostic_message = format_logs_error_message(&err);
            ui::command_finish("logs", false, &summary_message);
            CommandResult::failure("logs", started_at, diagnostic_message)
        }
    }
}

fn format_logs_error_message(err: &anyhow::Error) -> String {
    format_command_error("logs", err)
}

async fn run_async_with_target_override(
    args: LogsArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
) -> anyhow::Result<()> {
    let target_name = if target_override.is_some() {
        "override".to_string()
    } else {
        build::default_target_name().to_string()
    };
    let service_name = logs_service_for_context(args.name.as_deref());
    ui::command_stage("logs", "load-config", "loading target configuration");
    let target = match target_override {
        Some(target) => target.clone(),
        None => build::load_target_config(&target_name, project_root)
            .context("failed to load target configuration")?,
    }
    .require_deploy_credentials()
    .context("target settings are invalid for logs")?;
    ui::command_info(
        "logs",
        &format_local_context_line(
            project_root,
            service_name,
            &target_name,
            &target.remote,
            target.server_name.as_deref(),
        ),
    );
    ui::command_stage("logs", "connect", "connecting target");
    let connected = deploy::connect_target(&target).await?;
    ui::command_stage("logs", "hello", "negotiating hello");
    let hello = negotiate_hello_with_features(
        &connected.session,
        Uuid::new_v4(),
        &LOGS_HELLO_REQUIRED_FEATURES,
    )
    .await?;
    ui::command_info(
        "logs",
        &format_peer_context_line(
            &connected.authority,
            &connected.resolved_addr.to_string(),
            &hello,
        ),
    );

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
        deploy::response_payload(deploy::request_response(&connected.session, &request).await?)?;
    if !ack.accepted {
        return Err(anyhow!("logs.request was not accepted"));
    }
    if ack.names.is_empty() {
        return Err(anyhow!("logs.request returned no target service"));
    }

    receive_logs_datagrams(
        &connected.session,
        request_id,
        args.follow,
        args.name.is_none(),
    )
    .await?;
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
    let mut prefix_state = PrefixRenderState::default();

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
            break 'stream Ok(());
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
                if let Err(err) = render_chunk(&chunk, all_processes, &mut prefix_state) {
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
                break 'stream Ok(());
            }
        }
    }
}

async fn drain_post_end_chunks(
    session: &Session,
    request_id: Uuid,
    all_processes: bool,
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
        render_chunk(&chunk, all_processes, prefix_state)?;
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
        ui::command_warn("logs", "<<logs truncated>>");
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
    prefix_state: &mut PrefixRenderState,
) -> anyhow::Result<()> {
    if chunk.bytes.is_empty() {
        return Ok(());
    }

    render_text_chunk(chunk, all_processes, prefix_state)
}

fn render_text_chunk(
    chunk: &LogChunk,
    all_processes: bool,
    prefix_state: &mut PrefixRenderState,
) -> anyhow::Result<()> {
    let rendered = renderable_chunk_bytes(chunk, all_processes, prefix_state);
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
    prefix_state: &mut PrefixRenderState,
) -> Cow<'a, [u8]> {
    let _ = all_processes;
    let at_line_start = prefix_state.at_line_start(&chunk.name, chunk.stream_kind);
    let (rendered, next_at_line_start) =
        format_structured_bytes(&chunk.name, chunk.stream_kind, &chunk.bytes, at_line_start);
    prefix_state.set_at_line_start(&chunk.name, chunk.stream_kind, next_at_line_start);
    Cow::Owned(rendered)
}

fn format_structured_bytes(
    name: &str,
    stream_kind: LogStreamKind,
    bytes: &[u8],
    mut at_line_start: bool,
) -> (Vec<u8>, bool) {
    let mut out = Vec::new();

    let mut segment_start = 0usize;
    while segment_start < bytes.len() {
        if at_line_start {
            let prefix = format!("{} {} | ", name, stream_kind_label(stream_kind));
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
    fn format_structured_bytes_adds_prefix_for_each_newline_terminated_line() {
        let (rendered, at_line_start) =
            format_structured_bytes("svc-a", LogStreamKind::Stdout, b"a\nb\n", true);
        let rendered_text = String::from_utf8_lossy(&rendered);
        assert!(rendered_text.contains("svc-a stdout | a\n"));
        assert!(rendered_text.contains("svc-a stdout | b\n"));
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

        let first_rendered = renderable_chunk_bytes(&first, true, &mut prefix_state).into_owned();
        let first_text = String::from_utf8_lossy(&first_rendered);
        assert!(first_text.contains("svc-a stdout | hel"));
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

        let first_rendered = renderable_chunk_bytes(&first, false, &mut prefix_state).into_owned();
        let first_prefix_text = String::from_utf8_lossy(&first_rendered);
        assert!(first_prefix_text.contains("svc-a stdout | "));
        assert!(first_rendered.ends_with(&[0xe3, 0x81]));
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
    fn stderr_text_chunk_uses_stderr_only_for_single_process_output() {
        let chunk = LogChunk {
            request_id: Uuid::new_v4(),
            seq: 0,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Stderr,
            bytes: b"oops".to_vec(),
            is_last: false,
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
        };
        assert!(!should_write_text_chunk_to_stderr(&chunk, false));
        assert!(!should_write_text_chunk_to_stderr(&chunk, true));
    }

    #[test]
    fn logs_hello_required_features_are_fixed() {
        assert_eq!(LOGS_HELLO_REQUIRED_FEATURES, ["logs.request"]);
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
        assert!(message.contains("causes:"));
        assert!(message.contains("hints:"));
        assert!(message.contains("target settings"));
    }
}
