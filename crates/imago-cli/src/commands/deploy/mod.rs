use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow};
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushChunkHeader, ArtifactPushRequest,
    ArtifactStatus, ByteRange, CommandEvent, CommandEventType, CommandPayload, CommandStartRequest,
    CommandStartResponse, CommandType, DeployCommandPayload, DeployPrepareRequest,
    DeployPrepareResponse, ErrorCode, HelloNegotiateRequest, HelloNegotiateResponse, LogChunk,
    LogEnd, MessageType, PROTOCOL_VERSION, ProtocolEnvelope, StructuredError, from_cbor, to_cbor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader as TokioBufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinSet,
};
use uuid::Uuid;

use crate::{
    cli::{DeployArgs, LogsArgs},
    commands::{
        CommandResult, build, command_common, error_diagnostics, logs,
        shared::dependency::{DependencyResolver, StandardDependencyResolver},
        ui,
    },
    runtime,
};

mod artifact;
#[doc(hidden)]
pub mod network;

const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
const DEFAULT_MAX_INFLIGHT_CHUNKS: usize = 16;
const UPLOAD_MAX_ATTEMPTS: usize = 4;
const UPLOAD_RETRY_BASE_BACKOFF_MS: u64 = 250;
const UPLOAD_RETRY_MAX_BACKOFF_MS: u64 = 1000;
const DEFAULT_DEPLOY_STREAM_TIMEOUT_SECS: u64 = 30;
const DEPLOY_STREAM_RETRY_BACKOFF_MS: [u64; 2] = [100, 250];
const DEPLOY_STREAM_MAX_ATTEMPTS: usize = DEPLOY_STREAM_RETRY_BACKOFF_MS.len() + 1;
const DEPLOY_PHASE_TOTAL: u8 = 8;
const DEPLOY_PHASE_BUILD: u8 = 1;
const DEPLOY_PHASE_BUNDLE: u8 = 2;
const DEPLOY_PHASE_CONNECT: u8 = 3;
const DEPLOY_PHASE_HELLO: u8 = 4;
const DEPLOY_PHASE_PREPARE: u8 = 5;
const DEPLOY_PHASE_UPLOAD: u8 = 6;
const DEPLOY_PHASE_COMMIT: u8 = 7;
const DEPLOY_PHASE_COMMAND: u8 = 8;
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_RESET: &str = "\x1b[0m";
const DETAIL_WASM_STDOUT: &str = "wasm.stdout";
const DETAIL_WASM_STDERR: &str = "wasm.stderr";
const AUTO_FOLLOW_TAIL_LINES: u32 = 200;
const STDIO_MESSAGE_TERMINATOR: [u8; 4] = 0u32.to_be_bytes();
const LOCAL_PROXY_STDERR_CAPTURE_TIMEOUT_MS: u64 = 1000;

pub(crate) type Envelope = ProtocolEnvelope<Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestStreamRetryPolicy {
    Standard,
    CommandStartNoRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UploadLimits {
    chunk_size: usize,
    max_inflight_chunks: usize,
    deploy_stream_timeout: Duration,
}

#[derive(Clone, Copy)]
struct UploadRequestContext<'a> {
    session: &'a ConnectedTargetSession,
    correlation_id: Uuid,
    deploy_id: &'a str,
    upload_token: &'a str,
    stream_timeout: Duration,
}

struct UploadPhaseInputs<'a> {
    target: &'a build::DeployTargetConfig,
    target_for_protocol: &'a BTreeMap<String, String>,
    policy: &'a BTreeMap<String, String>,
    manifest: &'a Manifest,
    artifact_path: &'a Path,
    artifact_digest: &'a str,
    artifact_size: u64,
    manifest_digest: &'a str,
    idempotency_key: &'a str,
    correlation_id: Uuid,
}

struct UploadPhaseResult {
    session: ConnectedTargetSession,
    deploy_id: String,
    deploy_stream_timeout: Duration,
    authority: String,
    resolved_addr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeployRunSummary {
    service_name: String,
    deploy_id: String,
    target_name: String,
    authority: String,
    resolved: String,
    deployed_at: String,
}

#[derive(Clone)]
#[doc(hidden)]
pub struct ConnectedTargetSession {
    transport: Arc<dyn network::AdminTransport>,
    pub authority: String,
    pub resolved_addr: String,
    #[allow(dead_code)]
    pub configured_host: String,
    #[allow(dead_code)]
    pub remote_input: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum StreamRequestTermination {
    Completed,
    Interrupted,
}

struct SshTargetSession {
    remote: build::SshTargetRemote,
    remote_input: String,
    inner: tokio::sync::Mutex<ProcessIo>,
}

struct LocalProxyTargetSession {
    imagod_binary: PathBuf,
    socket_path: String,
    remote_input: String,
    inner: tokio::sync::Mutex<ProcessIo>,
}

#[cfg(unix)]
struct DirectSocketTargetSession {
    socket_path: String,
}

struct ProcessIo {
    child: Option<Child>,
    stdin: ChildStdin,
    stdout: TokioBufReader<ChildStdout>,
    stderr_capture: Option<LocalProxyStderrCapture>,
}

struct LocalProxyStderrCapture {
    bytes: Arc<StdMutex<Vec<u8>>>,
    drain_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
struct LocalProxyTransportError {
    summary: String,
    stderr_note: LocalProxyTransportStderrNote,
}

#[derive(Debug)]
struct LocalProxyTransportStderrNote {
    detail: String,
}

impl std::fmt::Display for LocalProxyTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary)
    }
}

impl std::error::Error for LocalProxyTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.stderr_note)
    }
}

impl std::fmt::Display for LocalProxyTransportStderrNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "local proxy transport stderr: {}", self.detail)
    }
}

impl std::error::Error for LocalProxyTransportStderrNote {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectedTargetMetadata {
    authority: String,
    resolved_addr: String,
    configured_host: String,
    remote_input: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultTargetTransportKind {
    Ssh,
    DirectSocket,
}

fn build_connected_target_metadata(
    target: &build::DeployTargetConfig,
    configured_host: &str,
    authority: &str,
    resolved_addr: impl Into<String>,
) -> ConnectedTargetMetadata {
    ConnectedTargetMetadata {
        authority: authority.to_string(),
        resolved_addr: resolved_addr.into(),
        configured_host: configured_host.to_string(),
        remote_input: target.remote.clone(),
    }
}

impl ConnectedTargetSession {
    pub(crate) fn close(&self, code: u32, reason: &[u8]) {
        let _ = (code, reason);
        self.transport.close();
    }
}

pub(crate) struct ConnectedSessionCloseGuard<'a> {
    session: Option<&'a ConnectedTargetSession>,
    reason: &'static [u8],
}

impl<'a> ConnectedSessionCloseGuard<'a> {
    pub(crate) fn new(session: &'a ConnectedTargetSession, reason: &'static [u8]) -> Self {
        Self {
            session: Some(session),
            reason,
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.session = None;
    }
}

impl Drop for ConnectedSessionCloseGuard<'_> {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            session.close(0, self.reason);
        }
    }
}

impl SshTargetSession {
    fn close_process(&self) {
        if let Ok(mut inner) = self.inner.try_lock() {
            terminate_process_and_reap(&mut inner.child);
        }
    }
}

impl LocalProxyTargetSession {
    fn close_process(&self) {
        if let Ok(mut inner) = self.inner.try_lock() {
            terminate_process_and_reap(&mut inner.child);
        }
    }
}

#[async_trait::async_trait]
impl network::AdminTransport for SshTargetSession {
    fn close(&self) {
        self.close_process();
    }

    async fn request_response_bytes(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_timeout: Option<Duration>,
    ) -> anyhow::Result<Vec<u8>> {
        request_events_over_ssh(self, framed, open_write_timeout, read_timeout).await
    }

    async fn stream_response_frames(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_idle_timeout: Option<Duration>,
        follow: bool,
        on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
    ) -> anyhow::Result<StreamRequestTermination> {
        request_streamed_frames_over_ssh(
            self,
            framed,
            open_write_timeout,
            read_idle_timeout,
            follow,
            on_frame,
        )
        .await
    }
}

#[async_trait::async_trait]
impl network::AdminTransport for LocalProxyTargetSession {
    fn close(&self) {
        self.close_process();
    }

    async fn request_response_bytes(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_timeout: Option<Duration>,
    ) -> anyhow::Result<Vec<u8>> {
        request_events_over_local_proxy(self, framed, open_write_timeout, read_timeout).await
    }

    async fn stream_response_frames(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_idle_timeout: Option<Duration>,
        follow: bool,
        on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
    ) -> anyhow::Result<StreamRequestTermination> {
        request_streamed_frames_over_local_proxy(
            self,
            framed,
            open_write_timeout,
            read_idle_timeout,
            follow,
            on_frame,
        )
        .await
    }
}

#[cfg(unix)]
#[async_trait::async_trait]
impl network::AdminTransport for DirectSocketTargetSession {
    fn close(&self) {}

    async fn request_response_bytes(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_timeout: Option<Duration>,
    ) -> anyhow::Result<Vec<u8>> {
        request_events_over_direct_socket(self, framed, open_write_timeout, read_timeout).await
    }

    async fn stream_response_frames(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_idle_timeout: Option<Duration>,
        follow: bool,
        on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
    ) -> anyhow::Result<StreamRequestTermination> {
        request_streamed_frames_over_direct_socket(
            self,
            framed,
            open_write_timeout,
            read_idle_timeout,
            follow,
            on_frame,
        )
        .await
    }
}

fn terminate_ssh_process(child: &mut Child) {
    let _ = child.start_kill();
}

fn terminate_process_and_reap(child: &mut Option<Child>) {
    let Some(mut child) = child.take() else {
        return;
    };
    terminate_ssh_process(&mut child);
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
}

fn replace_process_io(slot: &mut ProcessIo, replacement: ProcessIo) {
    let old = std::mem::replace(slot, replacement);
    let ProcessIo {
        mut child,
        stdin: _stdin,
        stdout: _stdout,
        stderr_capture: _stderr_capture,
    } = old;
    terminate_process_and_reap(&mut child);
}

fn format_local_proxy_stderr(stderr: &str) -> Option<String> {
    let lines = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    Some(lines.join(" | "))
}

#[cfg(unix)]
fn format_unix_socket_endpoint(socket_path: &str) -> String {
    format!("unix://{socket_path}")
}

#[cfg(unix)]
fn missing_imagod_socket_error(err: std::io::Error, socket_path: &str) -> anyhow::Error {
    Err::<(), std::io::Error>(err)
        .context(format!(
            "Cannot connect to the imagod daemon at {}. Is the imagod daemon running?",
            format_unix_socket_endpoint(socket_path)
        ))
        .expect_err("missing socket errors should remain failures")
}

async fn capture_local_proxy_stderr(inner: &mut ProcessIo) -> Option<String> {
    let mut child = inner.child.take()?;
    terminate_ssh_process(&mut child);
    let stderr_capture = inner.stderr_capture.take()?;
    let _ = tokio::time::timeout(
        Duration::from_millis(LOCAL_PROXY_STDERR_CAPTURE_TIMEOUT_MS),
        child.wait(),
    )
    .await;
    let _ = tokio::time::timeout(
        Duration::from_millis(LOCAL_PROXY_STDERR_CAPTURE_TIMEOUT_MS),
        stderr_capture.drain_task,
    )
    .await;
    let bytes = stderr_capture.bytes.lock().ok()?.clone();

    format_local_proxy_stderr(&String::from_utf8_lossy(&bytes))
}

fn annotate_local_proxy_transport_error(
    err: anyhow::Error,
    stderr: Option<String>,
) -> anyhow::Error {
    let Some(detail) = stderr else {
        return err;
    };
    anyhow::Error::new(LocalProxyTransportError {
        summary: err.to_string(),
        stderr_note: LocalProxyTransportStderrNote { detail },
    })
}

async fn reset_local_proxy_after_error<T>(
    inner: &mut ProcessIo,
    session: &LocalProxyTargetSession,
    err: anyhow::Error,
    reset_context: &'static str,
) -> anyhow::Result<T> {
    let err = annotate_local_proxy_transport_error(err, capture_local_proxy_stderr(inner).await);
    reset_local_proxy_process(
        inner,
        &session.imagod_binary,
        &session.socket_path,
        &session.remote_input,
    )
    .context(reset_context)?;
    Err(err)
}

fn deploy_phase_detail(phase: u8, detail: &str) -> String {
    format!("phase {phase}/{DEPLOY_PHASE_TOTAL} {detail}")
}

fn deploy_stage(phase: u8, stage: &str, detail: &str) {
    ui::command_stage("service.deploy", stage, &deploy_phase_detail(phase, detail));
}

fn format_deploy_build_preview(line: &build::BuildCommandLogLine) -> String {
    format!(
        "{} | {ANSI_DIM}  > [{}] {}{ANSI_RESET}",
        deploy_phase_detail(DEPLOY_PHASE_BUILD, "building project and manifest"),
        line.stream.as_str(),
        line.line
    )
}

fn format_build_failure_log(line: &build::BuildCommandLogLine) -> String {
    format!("  > {}", line.line)
}

fn build_failure_footer_line() -> &'static str {
    "build.command failed with errors; deploy aborted"
}

fn extract_build_failure_logs(err: &anyhow::Error) -> Option<&[build::BuildCommandLogLine]> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<build::BuildCommandFailure>())
        .map(|failure| failure.logs())
}

fn print_build_failure_logs(err: &anyhow::Error) {
    let Some(lines) = extract_build_failure_logs(err) else {
        return;
    };
    for line in lines {
        let _ = runtime::write_stdout_line(&format_build_failure_log(line));
    }
    let _ = runtime::write_stdout_line(build_failure_footer_line());
}

fn should_clear_deploy_spinner_before_follow(detach: bool) -> bool {
    !detach
}

fn format_deploy_structured_error(error: &StructuredError) -> String {
    let mut formatted = format!("{} ({:?}) at {}", error.message, error.code, error.stage);
    append_wasm_log_section(&mut formatted, error, DETAIL_WASM_STDOUT, "wasm stdout");
    append_wasm_log_section(&mut formatted, error, DETAIL_WASM_STDERR, "wasm stderr");
    formatted
}

fn append_wasm_log_section(
    formatted: &mut String,
    error: &StructuredError,
    detail_key: &str,
    section_label: &str,
) {
    let Some(detail) = error.details.get(detail_key) else {
        return;
    };
    if detail.is_empty() {
        return;
    }
    formatted.push('\n');
    formatted.push_str(section_label);
    formatted.push_str(":\n");
    formatted.push_str(detail);
}

#[derive(Debug, Clone)]
struct ServerResponseError {
    error: StructuredError,
}

impl std::fmt::Display for ServerResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "server error: {}",
            format_deploy_structured_error(&self.error)
        )
    }
}

impl std::error::Error for ServerResponseError {}

#[derive(Debug)]
struct CommitNotVerifiedError;

impl std::fmt::Display for CommitNotVerifiedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "artifact.commit returned verified=false")
    }
}

impl std::error::Error for CommitNotVerifiedError {}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    main: String,
    #[serde(rename = "type")]
    app_type: String,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
    #[serde(default)]
    dependencies: Vec<build::ManifestDependency>,
}

#[derive(Debug, Deserialize)]
struct ManifestAsset {
    path: String,
}

#[derive(Debug)]
struct TempArtifactBundle {
    path: PathBuf,
}

impl TempArtifactBundle {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempArtifactBundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn run(args: DeployArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(args: DeployArgs, project_root: &Path) -> CommandResult {
    run_with_project_root_and_target_override(args, project_root, None).await
}

pub(crate) async fn run_with_project_root_and_target_override(
    args: DeployArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("service.deploy", "starting");
    match run_async_with_target_override(args, project_root, target_override).await {
        Ok(summary) => {
            ui::command_finish("service.deploy", true, "");
            let mut result = CommandResult::success("service.deploy", started_at);
            result
                .meta
                .insert("service".to_string(), summary.service_name);
            result
                .meta
                .insert("deploy_id".to_string(), summary.deploy_id);
            result
                .meta
                .insert("target".to_string(), summary.target_name);
            result
                .meta
                .insert("authority".to_string(), summary.authority);
            result.meta.insert("resolved".to_string(), summary.resolved);
            result
                .meta
                .insert("deployed_at".to_string(), summary.deployed_at);
            result
        }
        Err(err) => {
            let summary = error_diagnostics::summarize_command_failure("service.deploy", &err);
            ui::command_finish("service.deploy", false, &summary);
            let message = error_diagnostics::format_command_error("service.deploy", &err);
            CommandResult::failure("service.deploy", started_at, message)
        }
    }
}

async fn run_async_with_target_override(
    args: DeployArgs,
    project_root: &Path,
    target_override: Option<&build::TargetConfig>,
) -> anyhow::Result<DeployRunSummary> {
    let DeployArgs { target, detach } = args;
    let dependency_resolver = StandardDependencyResolver;
    let target_connector = runtime::target_connector();
    let artifact_bundler = artifact::TarArtifactBundler;

    let target_name = target.unwrap_or_else(|| build::default_target_name().to_string());
    let service_name =
        build::load_service_name(project_root).unwrap_or_else(|_| "<unknown>".to_string());
    let resolved_target = match target_override {
        Some(target) => target.clone(),
        None => build::resolve_target_selector(&target_name, project_root)
            .context("failed to load target configuration")?,
    };
    if let Ok(context_target) = resolved_target.require_deploy_credentials() {
        ui::command_info(
            "service.deploy",
            &command_common::format_local_context_line(
                project_root,
                &service_name,
                &target_name,
                &context_target.remote,
            ),
        );
    }

    deploy_stage(DEPLOY_PHASE_BUILD, "build", "building project and manifest");
    let mut on_build_line = |line: &build::BuildCommandLogLine| {
        if ui::current_mode() != ui::UiMode::Rich {
            return;
        }
        ui::command_stage(
            "service.deploy",
            "build",
            &format_deploy_build_preview(line),
        );
    };
    let build_output = match build::build_project_with_target_override_for_deploy(
        &target_name,
        project_root,
        Some(&resolved_target),
        &mut on_build_line,
    ) {
        Ok(output) => output,
        Err(err) => {
            print_build_failure_logs(&err);
            return Err(err).context("build stage failed");
        }
    };

    let manifest_path = build_output.manifest_path;
    let manifest_bytes = build_output.manifest_bytes;
    let restart_policy = build_output.restart_policy;
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("failed to parse manifest json")?;
    let dependency_component_sources = dependency_resolver
        .resolve_dependency_component_sources(project_root, &manifest.dependencies)?;

    let target_config_for_logs = build_output.target.clone();
    let target = build_output
        .target
        .require_deploy_credentials()
        .context("target settings are invalid for service deploy")?;

    deploy_stage(
        DEPLOY_PHASE_BUNDLE,
        "bundle",
        "creating deploy artifact bundle",
    );
    let artifact = artifact::ArtifactBundler::bundle(
        &artifact_bundler,
        artifact::ArtifactBundleRequest {
            manifest: &manifest,
            manifest_path: &manifest_path,
            project_root,
            dependency_component_sources: &dependency_component_sources,
        },
    )?;
    let (artifact_digest, artifact_size) = compute_file_sha256_and_size(artifact.path())?;
    let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
    let correlation_id = Uuid::new_v4();
    let target_for_protocol = normalize_target_for_protocol(&target);
    let policy = BTreeMap::new();
    let idempotency_key = build_idempotency_key(
        &manifest.name,
        &manifest.app_type,
        &target_for_protocol,
        &policy,
        &artifact_digest,
        artifact_size,
        &manifest_digest,
    );

    let upload_result = run_upload_phase_with_resume(
        &*target_connector,
        UploadPhaseInputs {
            target: &target,
            target_for_protocol: &target_for_protocol,
            policy: &policy,
            manifest: &manifest,
            artifact_path: artifact.path(),
            artifact_digest: &artifact_digest,
            artifact_size,
            manifest_digest: &manifest_digest,
            idempotency_key: &idempotency_key,
            correlation_id,
        },
    )
    .await?;
    let mut session_close_guard =
        ConnectedSessionCloseGuard::new(&upload_result.session, b"service.deploy complete");

    deploy_stage(
        DEPLOY_PHASE_COMMAND,
        "command.start",
        "sending deploy command",
    );
    let command_request_id = Uuid::new_v4();
    let command = build_command_start_envelope(
        correlation_id,
        command_request_id,
        CommandType::Deploy,
        CommandPayload::Deploy(DeployCommandPayload {
            deploy_id: upload_result.deploy_id.clone(),
            expected_current_release: "any".to_string(),
            restart_policy,
            auto_rollback: true,
        }),
    )?;

    let responses = request_command_start_events_with_timeout(
        &upload_result.session,
        &command,
        upload_result.deploy_stream_timeout,
    )
    .await?;
    if responses.is_empty() {
        return Err(anyhow!("command.start returned empty response stream"));
    }

    let start_response: CommandStartResponse = response_payload(responses[0].clone())?;
    if !start_response.accepted {
        return Err(anyhow!("command.start was not accepted"));
    }

    let mut terminal: Option<CommandEvent> = None;
    for envelope in responses.iter().skip(1) {
        if envelope.message_type != MessageType::CommandEvent {
            continue;
        }
        let event: CommandEvent = response_payload(envelope.clone())?;
        if let Some(stage) = &event.stage {
            deploy_stage(DEPLOY_PHASE_COMMAND, stage, "remote progress");
        }
        if matches!(
            event.event_type,
            CommandEventType::Succeeded | CommandEventType::Failed | CommandEventType::Canceled
        ) {
            terminal = Some(event);
            break;
        }
    }

    let terminal =
        terminal.ok_or_else(|| anyhow!("command.event terminal event was not received"))?;
    match terminal.event_type {
        CommandEventType::Succeeded => {
            if should_clear_deploy_spinner_before_follow(detach) {
                session_close_guard.disarm();
                upload_result
                    .session
                    .close(0, b"service.deploy command session complete");
                ui::command_clear("service.deploy");
                follow_logs_after_deploy(project_root, &target_config_for_logs, &manifest.name)
                    .await;
            }
            Ok(DeployRunSummary {
                service_name: manifest.name.clone(),
                deploy_id: upload_result.deploy_id,
                target_name,
                authority: upload_result.authority,
                resolved: upload_result.resolved_addr,
                deployed_at: terminal.timestamp,
            })
        }
        CommandEventType::Failed => {
            if let Some(err) = terminal.error {
                Err(anyhow!(
                    "deploy failed: {}",
                    format_deploy_structured_error(&err)
                ))
            } else {
                Err(anyhow!("deploy failed without structured error"))
            }
        }
        CommandEventType::Canceled => Err(anyhow!("deploy was canceled")),
        _ => Err(anyhow!("unexpected terminal event")),
    }
}

async fn follow_logs_after_deploy(
    project_root: &Path,
    target_config: &build::TargetConfig,
    service_name: &str,
) {
    let logs_result = logs::run_with_project_root_and_target_override(
        LogsArgs {
            target: None,
            name: Some(service_name.to_string()),
            follow: true,
            tail: AUTO_FOLLOW_TAIL_LINES,
            with_timestamp: false,
        },
        project_root,
        Some(target_config),
    )
    .await;
    if logs_result.exit_code != 0 {
        let detail = logs_result
            .stderr
            .unwrap_or_else(|| format!("exit code {}", logs_result.exit_code));
        ui::command_warn(
            "service.deploy",
            &format!("service logs --follow failed after service deploy succeeded: {detail}"),
        );
    }
}

async fn run_upload_phase_with_resume<C: network::TargetConnector + ?Sized>(
    target_connector: &C,
    inputs: UploadPhaseInputs<'_>,
) -> anyhow::Result<UploadPhaseResult> {
    for attempt in 1..=UPLOAD_MAX_ATTEMPTS {
        match run_upload_phase_once(target_connector, &inputs).await {
            Ok(result) => return Ok(result),
            Err(err) => {
                if attempt >= UPLOAD_MAX_ATTEMPTS || !should_retry_upload_error(&err) {
                    return Err(err.context(format!(
                        "upload phase failed on attempt {attempt}/{UPLOAD_MAX_ATTEMPTS}"
                    )));
                }

                let backoff = retry_backoff_duration(attempt);
                ui::command_warn(
                    "service.deploy",
                    &format_retry_log_message(
                        attempt,
                        UPLOAD_MAX_ATTEMPTS,
                        backoff,
                        &summarize_retry_error(&err),
                    ),
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }

    Err(anyhow!(
        "upload retry loop exhausted unexpectedly without a terminal result"
    ))
}

async fn run_upload_phase_once<C: network::TargetConnector + ?Sized>(
    target_connector: &C,
    inputs: &UploadPhaseInputs<'_>,
) -> anyhow::Result<UploadPhaseResult> {
    deploy_stage(
        DEPLOY_PHASE_CONNECT,
        "connect",
        "establishing transport session",
    );
    let connected = target_connector.connect(inputs.target).await?;
    let mut session_close_guard =
        ConnectedSessionCloseGuard::new(&connected, b"service.deploy upload phase complete");

    deploy_stage(DEPLOY_PHASE_HELLO, "hello", "negotiating upload features");
    let hello = request_envelope(
        MessageType::HelloNegotiate,
        Uuid::new_v4(),
        inputs.correlation_id,
        &HelloNegotiateRequest {
            client_version: PROTOCOL_VERSION.to_string(),
            required_features: vec![
                "deploy.prepare".to_string(),
                "artifact.push".to_string(),
                "artifact.commit".to_string(),
                "command.start".to_string(),
                "command.event".to_string(),
            ],
        },
    )?;
    let hello_response: HelloNegotiateResponse =
        response_payload(request_response(&connected, &hello).await?)?;
    command_common::ensure_hello_protocol_compatibility(&hello_response)?;
    let hello_summary = hello_summary_from_response(&hello_response);
    ui::command_info(
        "service.deploy",
        &command_common::format_peer_context_line(
            &connected.authority,
            &connected.resolved_addr,
            &hello_summary,
        ),
    );
    let upload_limits = parse_upload_limits(&hello_response)?;

    deploy_stage(DEPLOY_PHASE_PREPARE, "prepare", "requesting deploy.prepare");
    let prepare = request_envelope(
        MessageType::DeployPrepare,
        Uuid::new_v4(),
        inputs.correlation_id,
        &DeployPrepareRequest {
            name: inputs.manifest.name.clone(),
            app_type: inputs.manifest.app_type.clone(),
            target: inputs.target_for_protocol.clone(),
            artifact_digest: inputs.artifact_digest.to_string(),
            artifact_size: inputs.artifact_size,
            manifest_digest: inputs.manifest_digest.to_string(),
            idempotency_key: inputs.idempotency_key.to_string(),
            policy: inputs.policy.clone(),
        },
    )?;
    let prepare_response: DeployPrepareResponse = response_payload(
        request_response_with_timeout(&connected, &prepare, upload_limits.deploy_stream_timeout)
            .await?,
    )?;

    let upload_ranges = upload_ranges_for_prepare(
        prepare_response.artifact_status,
        &prepare_response.missing_ranges,
        inputs.artifact_size,
    )?;
    if !upload_ranges.is_empty() {
        deploy_stage(DEPLOY_PHASE_UPLOAD, "upload", "uploading artifact");
        let upload_total_bytes = upload_ranges
            .iter()
            .fold(0u64, |acc, range| acc.saturating_add(range.length));
        let upload_detail = deploy_phase_detail(DEPLOY_PHASE_UPLOAD, "uploading artifact");
        ui::command_upload_start("service.deploy", upload_total_bytes, &upload_detail);
        let upload_context = UploadRequestContext {
            session: &connected,
            correlation_id: inputs.correlation_id,
            deploy_id: &prepare_response.deploy_id,
            upload_token: &prepare_response.upload_token,
            stream_timeout: upload_limits.deploy_stream_timeout,
        };
        push_artifact_ranges(
            upload_context,
            inputs.artifact_path,
            inputs.artifact_size,
            &upload_ranges,
            upload_limits,
        )
        .await?;
        ui::command_upload_finish("service.deploy");
    } else {
        deploy_stage(
            DEPLOY_PHASE_UPLOAD,
            "upload",
            "upload skipped (already present)",
        );
    }

    deploy_stage(DEPLOY_PHASE_COMMIT, "commit", "requesting artifact.commit");
    let commit = request_envelope(
        MessageType::ArtifactCommit,
        Uuid::new_v4(),
        inputs.correlation_id,
        &ArtifactCommitRequest {
            deploy_id: prepare_response.deploy_id.clone(),
            artifact_digest: inputs.artifact_digest.to_string(),
            artifact_size: inputs.artifact_size,
            manifest_digest: inputs.manifest_digest.to_string(),
        },
    )?;
    let commit_response: ArtifactCommitResponse = response_payload(
        request_response_with_timeout(&connected, &commit, upload_limits.deploy_stream_timeout)
            .await?,
    )?;
    if !commit_response.verified {
        return Err(CommitNotVerifiedError.into());
    }

    let authority = connected.authority.clone();
    let resolved_addr = connected.resolved_addr.clone();
    session_close_guard.disarm();
    drop(session_close_guard);
    Ok(UploadPhaseResult {
        session: connected,
        deploy_id: prepare_response.deploy_id,
        deploy_stream_timeout: upload_limits.deploy_stream_timeout,
        authority,
        resolved_addr,
    })
}

fn should_retry_upload_error(err: &anyhow::Error) -> bool {
    if contains_commit_not_verified_error(err) {
        return false;
    }

    match find_server_response_error(err) {
        Some(server_error) => {
            if is_non_retryable_error_code(server_error.error.code) {
                return false;
            }
            if server_error.error.code == ErrorCode::Busy {
                return true;
            }
            server_error.error.retryable
        }
        None => !contains_unauthorized_marker(err),
    }
}

fn retry_backoff_duration(retry_index: usize) -> Duration {
    let shift = retry_index.saturating_sub(1).min(8) as u32;
    let factor = 1u64 << shift;
    let millis = UPLOAD_RETRY_BASE_BACKOFF_MS
        .saturating_mul(factor)
        .min(UPLOAD_RETRY_MAX_BACKOFF_MS);
    Duration::from_millis(millis)
}

fn summarize_retry_error(err: &anyhow::Error) -> String {
    if let Some(server_error) = find_server_response_error(err) {
        return format!(
            "{:?} at {}",
            server_error.error.code, server_error.error.stage
        );
    }
    truncate_log_message(&err.to_string(), 160)
}

fn format_retry_log_message(
    attempt: usize,
    total_attempts: usize,
    backoff: Duration,
    reason: &str,
) -> String {
    format!(
        "upload attempt {attempt}/{total_attempts} failed, retrying in {}ms (reason={reason})",
        backoff.as_millis()
    )
}

fn truncate_log_message(message: &str, max_chars: usize) -> String {
    let len = message.chars().count();
    if len <= max_chars {
        return message.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let head = message
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    format!("{head}...")
}

fn find_server_response_error(err: &anyhow::Error) -> Option<&ServerResponseError> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<ServerResponseError>())
}

fn contains_commit_not_verified_error(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<CommitNotVerifiedError>().is_some())
}

fn is_non_retryable_error_code(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::Unauthorized
            | ErrorCode::BadRequest
            | ErrorCode::BadManifest
            | ErrorCode::IdempotencyConflict
            | ErrorCode::RangeInvalid
            | ErrorCode::ChunkHashMismatch
            | ErrorCode::StorageQuota
            | ErrorCode::PreconditionFailed
    )
}

fn contains_unauthorized_marker(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().contains("E_UNAUTHORIZED"))
}

pub(crate) async fn connect_target(
    target: &build::DeployTargetConfig,
) -> anyhow::Result<ConnectedTargetSession> {
    match default_target_transport_kind(&target.ssh_remote) {
        DefaultTargetTransportKind::Ssh => connect_ssh_target_only(target).await,
        #[cfg(unix)]
        DefaultTargetTransportKind::DirectSocket => connect_direct_socket_target(target),
        #[cfg(not(unix))]
        DefaultTargetTransportKind::DirectSocket => unreachable!("direct sockets are unix-only"),
    }
}

pub(crate) async fn connect_ssh_target_only(
    target: &build::DeployTargetConfig,
) -> anyhow::Result<ConnectedTargetSession> {
    connect_ssh_target(target, &target.ssh_remote).await
}

async fn connect_ssh_target(
    target: &build::DeployTargetConfig,
    remote: &build::SshTargetRemote,
) -> anyhow::Result<ConnectedTargetSession> {
    let authority = format_ssh_authority(remote);
    let resolved_addr = remote
        .socket_path
        .as_deref()
        .map(|path| format!("ssh:{} via {}", remote.host, path))
        .unwrap_or_else(|| format!("ssh:{} via /run/imago/imagod.sock", remote.host));
    let metadata = build_connected_target_metadata(target, &remote.host, &authority, resolved_addr);
    let process_io = spawn_ssh_proxy_process(remote, &target.remote)?;

    Ok(ConnectedTargetSession {
        transport: Arc::new(SshTargetSession {
            remote: remote.clone(),
            remote_input: target.remote.clone(),
            inner: tokio::sync::Mutex::new(process_io),
        }),
        authority: metadata.authority,
        resolved_addr: metadata.resolved_addr,
        configured_host: metadata.configured_host,
        remote_input: metadata.remote_input,
    })
}

#[cfg(unix)]
fn connect_direct_socket_target(
    target: &build::DeployTargetConfig,
) -> anyhow::Result<ConnectedTargetSession> {
    let socket_path = required_direct_socket_path(&target.ssh_remote)?;
    let authority = format_ssh_authority(&target.ssh_remote);
    let resolved_addr = format!("local-socket:{socket_path}");
    let metadata =
        build_connected_target_metadata(target, &target.ssh_remote.host, &authority, resolved_addr);

    Ok(ConnectedTargetSession {
        transport: Arc::new(DirectSocketTargetSession {
            socket_path: socket_path.to_string(),
        }),
        authority: metadata.authority,
        resolved_addr: metadata.resolved_addr,
        configured_host: metadata.configured_host,
        remote_input: metadata.remote_input,
    })
}

pub(crate) fn connect_local_proxy_target(
    target: &build::DeployTargetConfig,
    imagod_binary: &Path,
) -> anyhow::Result<ConnectedTargetSession> {
    let socket_path = required_local_proxy_socket_path(&target.ssh_remote)?;
    let authority = format_ssh_authority(&target.ssh_remote);
    let resolved_addr = format!("local-proxy:{} via {}", target.ssh_remote.host, socket_path);
    let metadata =
        build_connected_target_metadata(target, &target.ssh_remote.host, &authority, resolved_addr);
    let process_io = spawn_local_proxy_process(imagod_binary, socket_path, &target.remote)?;

    Ok(ConnectedTargetSession {
        transport: Arc::new(LocalProxyTargetSession {
            imagod_binary: imagod_binary.to_path_buf(),
            socket_path: socket_path.to_string(),
            remote_input: target.remote.clone(),
            inner: tokio::sync::Mutex::new(process_io),
        }),
        authority: metadata.authority,
        resolved_addr: metadata.resolved_addr,
        configured_host: metadata.configured_host,
        remote_input: metadata.remote_input,
    })
}

pub(crate) fn default_target_transport_kind(
    remote: &build::SshTargetRemote,
) -> DefaultTargetTransportKind {
    if should_use_direct_socket_target(remote) {
        DefaultTargetTransportKind::DirectSocket
    } else {
        DefaultTargetTransportKind::Ssh
    }
}

fn should_use_direct_socket_target(remote: &build::SshTargetRemote) -> bool {
    #[cfg(unix)]
    {
        is_loopback_ssh_target_host(&remote.host)
            && remote.user.is_none()
            && remote.port.is_none()
            && remote.socket_path.is_some()
    }

    #[cfg(not(unix))]
    {
        let _ = remote;
        false
    }
}

fn is_loopback_ssh_target_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn required_loopback_socket_path<'a>(
    remote: &'a build::SshTargetRemote,
    connector_label: &str,
) -> anyhow::Result<&'a str> {
    if !is_loopback_ssh_target_host(&remote.host) {
        return Err(anyhow!(
            "{connector_label} only supports loopback ssh targets, got '{}'",
            remote.host
        ));
    }

    remote
        .socket_path
        .as_deref()
        .ok_or_else(|| anyhow!("{connector_label} requires ?socket=/abs/path"))
}

fn required_local_proxy_socket_path(remote: &build::SshTargetRemote) -> anyhow::Result<&str> {
    required_loopback_socket_path(remote, "local proxy connector")
}

#[cfg(unix)]
fn required_direct_socket_path(remote: &build::SshTargetRemote) -> anyhow::Result<&str> {
    if remote.user.is_some() || remote.port.is_some() {
        return Err(anyhow!(
            "direct socket connector only supports loopback ssh targets without user/port overrides"
        ));
    }
    required_loopback_socket_path(remote, "direct socket connector")
}

fn spawn_ssh_proxy_process(
    remote: &build::SshTargetRemote,
    remote_input: &str,
) -> anyhow::Result<ProcessIo> {
    let mut command = Command::new("ssh");
    command.args(ssh_proxy_command_args(remote));
    spawn_process(
        command,
        remote_input,
        "ssh transport",
        "ssh stdin",
        "ssh stdout",
        false,
        None,
    )
}

fn spawn_local_proxy_process(
    imagod_binary: &Path,
    socket_path: &str,
    remote_input: &str,
) -> anyhow::Result<ProcessIo> {
    let mut command = Command::new(imagod_binary);
    command.arg("proxy-stdio");
    command.arg("--socket");
    command.arg(socket_path);
    spawn_process(
        command,
        remote_input,
        "local proxy transport",
        "local proxy stdin",
        "local proxy stdout",
        true,
        Some("local proxy stderr"),
    )
}

fn spawn_process(
    mut command: Command,
    remote_input: &str,
    label: &str,
    stdin_label: &str,
    stdout_label: &str,
    capture_stderr: bool,
    stderr_label: Option<&str>,
) -> anyhow::Result<ProcessIo> {
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    if capture_stderr {
        command.stderr(std::process::Stdio::piped());
    } else {
        command.stderr(std::process::Stdio::inherit());
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn {label} for {remote_input}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("failed to capture {stdin_label}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture {stdout_label}"))?;
    let stderr_capture = if capture_stderr {
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("failed to capture {}", stderr_label.unwrap_or("stderr")))?;
        let bytes = Arc::new(StdMutex::new(Vec::new()));
        let task_bytes = bytes.clone();
        let drain_task = tokio::spawn(async move {
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if let Ok(mut guard) = task_bytes.lock() {
                            guard.extend_from_slice(&chunk[..read]);
                        }
                    }
                }
            }
        });
        Some(LocalProxyStderrCapture { bytes, drain_task })
    } else {
        None
    };

    Ok(ProcessIo {
        child: Some(child),
        stdin,
        stdout: TokioBufReader::new(stdout),
        stderr_capture,
    })
}

fn ssh_proxy_command_args(remote: &build::SshTargetRemote) -> Vec<String> {
    let mut args = vec![
        "-T".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
    ];
    if let Some(port) = remote.port {
        args.push("-p".to_string());
        args.push(port.to_string());
    }
    args.push(match remote.user.as_deref() {
        Some(user) => format!("{user}@{}", remote.host),
        None => remote.host.clone(),
    });
    args.push("imagod".to_string());
    args.push("proxy-stdio".to_string());
    if let Some(socket_path) = remote.socket_path.as_deref() {
        args.push("--socket".to_string());
        args.push(socket_path.to_string());
    }
    args
}

fn format_ssh_authority(remote: &build::SshTargetRemote) -> String {
    let authority_host = match remote.user.as_deref() {
        Some(user) => format!("{user}@{}", remote.host),
        None => remote.host.clone(),
    };
    if let Some(port) = remote.port {
        format!("ssh://{authority_host}:{port}")
    } else {
        format!("ssh://{authority_host}")
    }
}

fn reset_ssh_process(
    inner: &mut ProcessIo,
    remote: &build::SshTargetRemote,
    remote_input: &str,
) -> anyhow::Result<()> {
    let replacement = spawn_ssh_proxy_process(remote, remote_input)?;
    replace_process_io(inner, replacement);
    Ok(())
}

fn reset_local_proxy_process(
    inner: &mut ProcessIo,
    imagod_binary: &Path,
    socket_path: &str,
    remote_input: &str,
) -> anyhow::Result<()> {
    let replacement = spawn_local_proxy_process(imagod_binary, socket_path, remote_input)?;
    replace_process_io(inner, replacement);
    Ok(())
}

fn recover_ssh_read_result<T, F>(result: anyhow::Result<T>, mut reset: F) -> anyhow::Result<T>
where
    F: FnMut() -> anyhow::Result<()>,
{
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            reset().context("failed to reset ssh transport after read failure")?;
            Err(err)
        }
    }
}

fn hello_summary_from_response(response: &HelloNegotiateResponse) -> command_common::HelloSummary {
    command_common::HelloSummary {
        server_version: response.server_version.clone(),
        features: response.features.clone(),
        limits: response.limits.clone(),
    }
}

fn parse_upload_limits(response: &HelloNegotiateResponse) -> anyhow::Result<UploadLimits> {
    let chunk_size = parse_positive_limit(
        &response.limits,
        "chunk_size",
        DEFAULT_CHUNK_SIZE,
        "hello.negotiate response chunk_size",
    )?;
    let max_inflight_chunks = parse_positive_limit(
        &response.limits,
        "max_inflight_chunks",
        DEFAULT_MAX_INFLIGHT_CHUNKS,
        "hello.negotiate response max_inflight_chunks",
    )?;
    let deploy_stream_timeout_secs = parse_positive_limit_u64(
        &response.limits,
        "deploy_stream_timeout_secs",
        DEFAULT_DEPLOY_STREAM_TIMEOUT_SECS,
        "hello.negotiate response deploy_stream_timeout_secs",
    )?;

    Ok(UploadLimits {
        chunk_size,
        max_inflight_chunks,
        deploy_stream_timeout: Duration::from_secs(deploy_stream_timeout_secs),
    })
}

fn command_stream_timeout_from_hello_limits(
    limits: &BTreeMap<String, String>,
    default_timeout: Duration,
) -> Duration {
    limits
        .get("deploy_stream_timeout_secs")
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(default_timeout)
}

pub(crate) fn resolve_command_stream_timeout_from_hello_limits(
    limits: &BTreeMap<String, String>,
) -> Duration {
    command_stream_timeout_from_hello_limits(limits, resolve_deploy_stream_timeout())
}

fn parse_positive_limit(
    limits: &BTreeMap<String, String>,
    key: &str,
    default: usize,
    label: &str,
) -> anyhow::Result<usize> {
    match limits.get(key) {
        Some(raw) => {
            let parsed = raw
                .parse::<usize>()
                .with_context(|| format!("failed to parse {label} as integer: {raw}"))?;
            if parsed == 0 {
                return Err(anyhow!("{label} must be greater than 0"));
            }
            Ok(parsed)
        }
        None => Ok(default),
    }
}

fn parse_positive_limit_u64(
    limits: &BTreeMap<String, String>,
    key: &str,
    default: u64,
    label: &str,
) -> anyhow::Result<u64> {
    match limits.get(key) {
        Some(raw) => {
            let parsed = raw
                .parse::<u64>()
                .with_context(|| format!("failed to parse {label} as integer: {raw}"))?;
            if parsed == 0 {
                return Err(anyhow!("{label} must be greater than 0"));
            }
            Ok(parsed)
        }
        None => Ok(default),
    }
}

pub(crate) fn request_envelope<T: Serialize>(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    payload: &T,
) -> anyhow::Result<Envelope> {
    Ok(Envelope {
        message_type,
        request_id,
        correlation_id,
        payload: serde_json::to_value(payload)?,
        error: None,
    })
}

pub(crate) fn build_command_start_envelope(
    correlation_id: Uuid,
    request_id: Uuid,
    command_type: CommandType,
    payload: CommandPayload,
) -> anyhow::Result<Envelope> {
    request_envelope(
        MessageType::CommandStart,
        request_id,
        correlation_id,
        &CommandStartRequest {
            request_id,
            command_type,
            payload,
        },
    )
}

pub(crate) async fn request_response(
    session: &ConnectedTargetSession,
    envelope: &Envelope,
) -> anyhow::Result<Envelope> {
    request_response_with_timeout(session, envelope, resolve_deploy_stream_timeout()).await
}

pub(crate) async fn request_response_with_timeout(
    session: &ConnectedTargetSession,
    envelope: &Envelope,
    stream_timeout: Duration,
) -> anyhow::Result<Envelope> {
    let responses = request_events_with_timeout(session, envelope, stream_timeout).await?;
    responses
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty response stream"))
}

pub(crate) async fn request_command_start_events_with_timeout(
    session: &ConnectedTargetSession,
    envelope: &Envelope,
    stream_timeout: Duration,
) -> anyhow::Result<Vec<Envelope>> {
    request_events_with_retry_policy(
        session,
        envelope,
        stream_timeout,
        RequestStreamRetryPolicy::CommandStartNoRetry,
    )
    .await
    .map_err(|err| {
        err.context(
            "command.start request stream failed; command may still be running on target. wait for in-flight operations to finish and inspect logs/state before retrying",
        )
    })
}

pub(crate) async fn request_events_with_timeout(
    session: &ConnectedTargetSession,
    envelope: &Envelope,
    stream_timeout: Duration,
) -> anyhow::Result<Vec<Envelope>> {
    request_events_with_retry_policy(
        session,
        envelope,
        stream_timeout,
        RequestStreamRetryPolicy::Standard,
    )
    .await
}

async fn request_events_with_retry_policy(
    session: &ConnectedTargetSession,
    envelope: &Envelope,
    stream_timeout: Duration,
    retry_policy: RequestStreamRetryPolicy,
) -> anyhow::Result<Vec<Envelope>> {
    let payload = to_cbor(envelope)?;
    let framed = encode_frame(&payload);
    request_events_with_retry_policy_framed(session, &framed, stream_timeout, retry_policy).await
}

async fn request_events_with_retry_policy_framed(
    session: &ConnectedTargetSession,
    framed: &[u8],
    stream_timeout: Duration,
    retry_policy: RequestStreamRetryPolicy,
) -> anyhow::Result<Vec<Envelope>> {
    let mut attempt = 1usize;
    let mut first_failure_reason: Option<String> = None;
    let read_timeout = request_stream_read_timeout(retry_policy, stream_timeout);
    let response_bytes = loop {
        match request_events_once(session, framed, stream_timeout, read_timeout).await {
            Ok(response_bytes) => break response_bytes,
            Err(err) => {
                let reason = summarize_retry_error(&err);
                if first_failure_reason.is_none() {
                    first_failure_reason = Some(reason.clone());
                }
                let Some(backoff) = request_stream_retry_backoff(retry_policy, attempt) else {
                    let detail = format_request_stream_failure_summary(
                        attempt,
                        first_failure_reason.as_deref(),
                        &reason,
                    );
                    return Err(err.context(detail));
                };
                ui::command_warn(
                    "service.deploy",
                    &format_request_stream_retry_log_message(
                        attempt,
                        request_stream_max_attempts(retry_policy),
                        backoff,
                        &reason,
                    ),
                );
                tokio::time::sleep(backoff).await;
                attempt = attempt.saturating_add(1);
            }
        }
    };
    let frames = decode_frames(&response_bytes)?;
    let mut envelopes = Vec::with_capacity(frames.len());
    for frame in frames {
        envelopes.push(decode_response_envelope(&frame)?);
    }
    Ok(envelopes)
}

async fn request_events_once(
    session: &ConnectedTargetSession,
    framed: &[u8],
    open_write_timeout: Duration,
    read_timeout: Option<Duration>,
) -> anyhow::Result<Vec<u8>> {
    session
        .transport
        .request_response_bytes(framed, open_write_timeout, read_timeout)
        .await
}

async fn request_events_over_ssh(
    session: &SshTargetSession,
    framed: &[u8],
    open_write_timeout: Duration,
    read_timeout: Option<Duration>,
) -> anyhow::Result<Vec<u8>> {
    let mut inner = session.inner.lock().await;
    tokio::time::timeout(open_write_timeout, inner.stdin.write_all(framed))
        .await
        .map_err(|_| {
            anyhow!(
                "ssh transport write timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;
    tokio::time::timeout(
        open_write_timeout,
        inner.stdin.write_all(&STDIO_MESSAGE_TERMINATOR),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "ssh transport write timed out after {} ms",
            open_write_timeout.as_millis()
        )
    })??;
    tokio::time::timeout(open_write_timeout, inner.stdin.flush())
        .await
        .map_err(|_| {
            anyhow!(
                "ssh transport flush timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;

    match read_timeout {
        Some(read_timeout) => {
            match tokio::time::timeout(
                read_timeout,
                read_stdio_response_message_with_label(&mut inner.stdout, "ssh transport"),
            )
            .await
            {
                Ok(result) => recover_ssh_read_result(result, || {
                    reset_ssh_process(&mut inner, &session.remote, &session.remote_input)
                }),
                Err(_) => {
                    reset_ssh_process(&mut inner, &session.remote, &session.remote_input)?;
                    Err(anyhow!(
                        "ssh transport read timed out after {} ms",
                        read_timeout.as_millis()
                    ))
                }
            }
        }
        None => recover_ssh_read_result(
            read_stdio_response_message_with_label(&mut inner.stdout, "ssh transport").await,
            || reset_ssh_process(&mut inner, &session.remote, &session.remote_input),
        ),
    }
}

async fn request_events_over_local_proxy(
    session: &LocalProxyTargetSession,
    framed: &[u8],
    open_write_timeout: Duration,
    read_timeout: Option<Duration>,
) -> anyhow::Result<Vec<u8>> {
    let mut inner = session.inner.lock().await;
    let result = async {
        tokio::time::timeout(open_write_timeout, inner.stdin.write_all(framed))
            .await
            .map_err(|_| {
                anyhow!(
                    "local proxy transport write timed out after {} ms",
                    open_write_timeout.as_millis()
                )
            })??;
        tokio::time::timeout(
            open_write_timeout,
            inner.stdin.write_all(&STDIO_MESSAGE_TERMINATOR),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "local proxy transport write timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;
        tokio::time::timeout(open_write_timeout, inner.stdin.flush())
            .await
            .map_err(|_| {
                anyhow!(
                    "local proxy transport flush timed out after {} ms",
                    open_write_timeout.as_millis()
                )
            })??;

        match read_timeout {
            Some(read_timeout) => {
                match tokio::time::timeout(
                    read_timeout,
                    read_stdio_response_message_with_label(
                        &mut inner.stdout,
                        "local proxy transport",
                    ),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(anyhow!(
                        "local proxy transport read timed out after {} ms",
                        read_timeout.as_millis()
                    )),
                }
            }
            None => {
                read_stdio_response_message_with_label(&mut inner.stdout, "local proxy transport")
                    .await
            }
        }
    }
    .await;

    match result {
        Ok(response) => Ok(response),
        Err(err) => {
            reset_local_proxy_after_error(
                &mut inner,
                session,
                err,
                "failed to reset local proxy transport after request failure",
            )
            .await
        }
    }
}

#[cfg(unix)]
async fn connect_direct_socket_with_timeout(
    session: &DirectSocketTargetSession,
    open_write_timeout: Duration,
) -> anyhow::Result<tokio::net::UnixStream> {
    tokio::time::timeout(
        open_write_timeout,
        tokio::net::UnixStream::connect(&session.socket_path),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "local socket transport connect timed out after {} ms",
            open_write_timeout.as_millis()
        )
    })?
    .map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            missing_imagod_socket_error(err, &session.socket_path)
        } else {
            anyhow::Error::from(err).context(format!(
                "local socket transport connect failed for {}",
                session.socket_path
            ))
        }
    })
}

#[cfg(unix)]
async fn request_events_over_direct_socket(
    session: &DirectSocketTargetSession,
    framed: &[u8],
    open_write_timeout: Duration,
    read_timeout: Option<Duration>,
) -> anyhow::Result<Vec<u8>> {
    let mut stream = connect_direct_socket_with_timeout(session, open_write_timeout).await?;
    tokio::time::timeout(open_write_timeout, stream.write_all(framed))
        .await
        .map_err(|_| {
            anyhow!(
                "local socket transport write timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;
    tokio::time::timeout(
        open_write_timeout,
        stream.write_all(&STDIO_MESSAGE_TERMINATOR),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "local socket transport write timed out after {} ms",
            open_write_timeout.as_millis()
        )
    })??;
    tokio::time::timeout(open_write_timeout, stream.flush())
        .await
        .map_err(|_| {
            anyhow!(
                "local socket transport flush timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;

    // Local control socket requests are stdio-terminated, but responses finish on EOF.
    // `imagod proxy-stdio` is the adapter that converts socket EOF into a stdio terminator.
    match read_timeout {
        Some(read_timeout) => {
            match tokio::time::timeout(
                read_timeout,
                read_direct_socket_response_message_with_label(
                    &mut stream,
                    "local socket transport",
                ),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow!(
                    "local socket transport read timed out after {} ms",
                    read_timeout.as_millis()
                )),
            }
        }
        None => {
            read_direct_socket_response_message_with_label(&mut stream, "local socket transport")
                .await
        }
    }
}

async fn request_streamed_frames_over_ssh(
    session: &SshTargetSession,
    framed: &[u8],
    open_write_timeout: Duration,
    read_idle_timeout: Option<Duration>,
    follow: bool,
    on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
) -> anyhow::Result<StreamRequestTermination> {
    let mut inner = session.inner.lock().await;
    tokio::time::timeout(open_write_timeout, inner.stdin.write_all(framed))
        .await
        .map_err(|_| {
            anyhow!(
                "ssh transport write timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;
    tokio::time::timeout(
        open_write_timeout,
        inner.stdin.write_all(&STDIO_MESSAGE_TERMINATOR),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "ssh transport write timed out after {} ms",
            open_write_timeout.as_millis()
        )
    })??;
    tokio::time::timeout(open_write_timeout, inner.stdin.flush())
        .await
        .map_err(|_| {
            anyhow!(
                "ssh transport flush timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;

    loop {
        let next = if follow {
            tokio::select! {
                frame = read_next_stdio_response_frame_with_label(&mut inner.stdout, "ssh transport") => Some(frame),
                _ = tokio::signal::ctrl_c() => None,
            }
        } else if let Some(read_idle_timeout) = read_idle_timeout {
            match tokio::time::timeout(
                read_idle_timeout,
                read_next_stdio_response_frame_with_label(&mut inner.stdout, "ssh transport"),
            )
            .await
            {
                Ok(result) => Some(result),
                Err(_) => {
                    reset_ssh_process(&mut inner, &session.remote, &session.remote_input)?;
                    return Err(anyhow!(
                        "ssh transport read timed out after {} ms",
                        read_idle_timeout.as_millis()
                    ));
                }
            }
        } else {
            Some(
                read_next_stdio_response_frame_with_label(&mut inner.stdout, "ssh transport").await,
            )
        };
        let Some(next) = next else {
            terminate_process_and_reap(&mut inner.child);
            return Ok(StreamRequestTermination::Interrupted);
        };
        let next = recover_ssh_read_result(next, || {
            reset_ssh_process(&mut inner, &session.remote, &session.remote_input)
        })?;
        let Some(frame) = next else {
            return Ok(StreamRequestTermination::Completed);
        };
        if on_frame(frame)? {
            return Ok(StreamRequestTermination::Completed);
        }
    }
}

async fn request_streamed_frames_over_local_proxy(
    session: &LocalProxyTargetSession,
    framed: &[u8],
    open_write_timeout: Duration,
    read_idle_timeout: Option<Duration>,
    follow: bool,
    on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
) -> anyhow::Result<StreamRequestTermination> {
    let mut inner = session.inner.lock().await;
    let write_result: anyhow::Result<()> = async {
        tokio::time::timeout(open_write_timeout, inner.stdin.write_all(framed))
            .await
            .map_err(|_| {
                anyhow!(
                    "local proxy transport write timed out after {} ms",
                    open_write_timeout.as_millis()
                )
            })??;
        tokio::time::timeout(
            open_write_timeout,
            inner.stdin.write_all(&STDIO_MESSAGE_TERMINATOR),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "local proxy transport write timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;
        tokio::time::timeout(open_write_timeout, inner.stdin.flush())
            .await
            .map_err(|_| {
                anyhow!(
                    "local proxy transport flush timed out after {} ms",
                    open_write_timeout.as_millis()
                )
            })??;
        Ok(())
    }
    .await;
    if let Err(err) = write_result {
        return reset_local_proxy_after_error(
            &mut inner,
            session,
            err,
            "failed to reset local proxy transport after request failure",
        )
        .await;
    }

    loop {
        let next = if follow {
            tokio::select! {
                frame = read_next_stdio_response_frame_with_label(&mut inner.stdout, "local proxy transport") => Some(frame),
                _ = tokio::signal::ctrl_c() => None,
            }
        } else if let Some(read_idle_timeout) = read_idle_timeout {
            match tokio::time::timeout(
                read_idle_timeout,
                read_next_stdio_response_frame_with_label(
                    &mut inner.stdout,
                    "local proxy transport",
                ),
            )
            .await
            {
                Ok(result) => Some(result),
                Err(_) => {
                    return reset_local_proxy_after_error(
                        &mut inner,
                        session,
                        anyhow!(
                            "local proxy transport read timed out after {} ms",
                            read_idle_timeout.as_millis()
                        ),
                        "failed to reset local proxy transport after request failure",
                    )
                    .await;
                }
            }
        } else {
            Some(
                read_next_stdio_response_frame_with_label(
                    &mut inner.stdout,
                    "local proxy transport",
                )
                .await,
            )
        };
        let Some(next) = next else {
            terminate_process_and_reap(&mut inner.child);
            return Ok(StreamRequestTermination::Interrupted);
        };
        let next = match next {
            Ok(next) => next,
            Err(err) => {
                return reset_local_proxy_after_error(
                    &mut inner,
                    session,
                    err,
                    "failed to reset local proxy transport after request failure",
                )
                .await;
            }
        };
        let Some(frame) = next else {
            return Ok(StreamRequestTermination::Completed);
        };
        if on_frame(frame)? {
            return Ok(StreamRequestTermination::Completed);
        }
    }
}

#[cfg(unix)]
async fn request_streamed_frames_over_direct_socket(
    session: &DirectSocketTargetSession,
    framed: &[u8],
    open_write_timeout: Duration,
    read_idle_timeout: Option<Duration>,
    follow: bool,
    on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
) -> anyhow::Result<StreamRequestTermination> {
    let mut stream = connect_direct_socket_with_timeout(session, open_write_timeout).await?;
    tokio::time::timeout(open_write_timeout, stream.write_all(framed))
        .await
        .map_err(|_| {
            anyhow!(
                "local socket transport write timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;
    tokio::time::timeout(
        open_write_timeout,
        stream.write_all(&STDIO_MESSAGE_TERMINATOR),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "local socket transport write timed out after {} ms",
            open_write_timeout.as_millis()
        )
    })??;
    tokio::time::timeout(open_write_timeout, stream.flush())
        .await
        .map_err(|_| {
            anyhow!(
                "local socket transport flush timed out after {} ms",
                open_write_timeout.as_millis()
            )
        })??;

    loop {
        let next = if follow {
            tokio::select! {
                frame = read_next_direct_socket_response_frame_with_label(&mut stream, "local socket transport") => Some(frame),
                _ = tokio::signal::ctrl_c() => None,
            }
        } else if let Some(read_idle_timeout) = read_idle_timeout {
            match tokio::time::timeout(
                read_idle_timeout,
                read_next_direct_socket_response_frame_with_label(
                    &mut stream,
                    "local socket transport",
                ),
            )
            .await
            {
                Ok(result) => Some(result),
                Err(_) => {
                    return Err(anyhow!(
                        "local socket transport read timed out after {} ms",
                        read_idle_timeout.as_millis()
                    ));
                }
            }
        } else {
            Some(
                read_next_direct_socket_response_frame_with_label(
                    &mut stream,
                    "local socket transport",
                )
                .await,
            )
        };
        let Some(next) = next else {
            return Ok(StreamRequestTermination::Interrupted);
        };
        let Some(frame) = next? else {
            return Ok(StreamRequestTermination::Completed);
        };
        if on_frame(frame)? {
            return Ok(StreamRequestTermination::Completed);
        }
    }
}

#[cfg(unix)]
async fn read_direct_socket_response_message_with_label<R>(
    reader: &mut R,
    transport_label: &str,
) -> anyhow::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut out = Vec::new();
    loop {
        let Some(payload_len) =
            read_next_direct_socket_response_frame_len_with_label(reader, transport_label).await?
        else {
            break;
        };
        ensure_stdio_response_message_growth_with_label(out.len(), payload_len, transport_label)?;
        let payload =
            read_direct_socket_response_payload_with_label(reader, payload_len, transport_label)
                .await?;
        out.extend_from_slice(&encode_frame(&payload));
    }
    Ok(out)
}

async fn read_stdio_response_message_with_label<R>(
    reader: &mut R,
    transport_label: &str,
) -> anyhow::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut out = Vec::new();
    loop {
        let mut header = [0u8; 4];
        match reader.read_exact(&mut header).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof && out.is_empty() => {
                return Err(anyhow!(
                    "{transport_label} closed before returning a response"
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(anyhow!(
                    "{transport_label} closed in the middle of a response"
                ));
            }
            Err(err) => return Err(anyhow::Error::from(err)),
        }

        let len = u32::from_be_bytes(header) as usize;
        if len == 0 {
            break;
        }
        ensure_stdio_response_message_growth_with_label(out.len(), len, transport_label)?;

        let mut payload = vec![0u8; len];
        reader.read_exact(&mut payload).await?;
        out.extend_from_slice(&encode_frame(&payload));
    }
    Ok(out)
}

fn ensure_stdio_response_message_growth_with_label(
    current_len: usize,
    payload_len: usize,
    transport_label: &str,
) -> anyhow::Result<()> {
    ensure_stdio_response_frame_len_with_label(payload_len, transport_label)?;
    let next_frame_len = 4usize.checked_add(payload_len).ok_or_else(|| {
        anyhow!("{transport_label} response exceeds max size {MAX_STREAM_BYTES} bytes")
    })?;
    let projected_len = current_len.checked_add(next_frame_len).ok_or_else(|| {
        anyhow!("{transport_label} response exceeds max size {MAX_STREAM_BYTES} bytes")
    })?;
    if projected_len > MAX_STREAM_BYTES {
        return Err(anyhow!(
            "{transport_label} response exceeds max size {MAX_STREAM_BYTES} bytes"
        ));
    }
    Ok(())
}

pub(crate) async fn request_streamed_events<F>(
    session: &ConnectedTargetSession,
    envelope: &Envelope,
    open_write_timeout: Duration,
    read_idle_timeout: Option<Duration>,
    follow: bool,
    mut on_envelope: F,
) -> anyhow::Result<StreamRequestTermination>
where
    F: FnMut(Envelope) -> anyhow::Result<bool> + Send,
{
    let payload = to_cbor(envelope)?;
    let framed = encode_frame(&payload);
    let mut on_frame = |frame: Vec<u8>| on_envelope(decode_response_envelope(&frame)?);
    session
        .transport
        .stream_response_frames(
            &framed,
            open_write_timeout,
            read_idle_timeout,
            follow,
            &mut on_frame,
        )
        .await
}

#[cfg(unix)]
async fn read_next_direct_socket_response_frame_with_label<R>(
    reader: &mut R,
    transport_label: &str,
) -> anyhow::Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let Some(payload_len) =
        read_next_direct_socket_response_frame_len_with_label(reader, transport_label).await?
    else {
        return Ok(None);
    };
    let payload =
        read_direct_socket_response_payload_with_label(reader, payload_len, transport_label)
            .await?;
    Ok(Some(payload))
}

#[cfg(unix)]
async fn read_next_direct_socket_response_frame_len_with_label<R>(
    reader: &mut R,
    transport_label: &str,
) -> anyhow::Result<Option<usize>>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; 4];
    let mut read = 0usize;
    while read < header.len() {
        match reader.read(&mut header[read..]).await {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => {
                return Err(anyhow!(
                    "{transport_label} closed in the middle of a response"
                ));
            }
            Ok(chunk) => {
                read = read.saturating_add(chunk);
            }
            Err(err) => return Err(anyhow::Error::from(err)),
        }
    }

    let len = u32::from_be_bytes(header) as usize;
    if len == 0 {
        return Ok(None);
    }
    ensure_stdio_response_frame_len_with_label(len, transport_label)?;
    Ok(Some(len))
}

async fn read_next_stdio_response_frame_with_label<R>(
    reader: &mut R,
    transport_label: &str,
) -> anyhow::Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; 4];
    reader.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 {
        return Ok(None);
    }
    ensure_stdio_response_frame_len_with_label(len, transport_label)?;
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

#[cfg(unix)]
async fn read_direct_socket_response_payload_with_label<R>(
    reader: &mut R,
    len: usize,
    transport_label: &str,
) -> anyhow::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut payload = vec![0u8; len];
    let mut read = 0usize;
    while read < len {
        match reader.read(&mut payload[read..]).await {
            Ok(0) => {
                return Err(anyhow!(
                    "{transport_label} closed in the middle of a response"
                ));
            }
            Ok(chunk) => {
                read = read.saturating_add(chunk);
            }
            Err(err) => return Err(anyhow::Error::from(err)),
        }
    }
    Ok(payload)
}

fn ensure_stdio_response_frame_len_with_label(
    len: usize,
    transport_label: &str,
) -> anyhow::Result<()> {
    if len > MAX_STREAM_BYTES {
        return Err(anyhow!(
            "{transport_label} response frame exceeds max size {MAX_STREAM_BYTES} bytes"
        ));
    }
    Ok(())
}

fn deploy_stream_retry_backoff(attempt: usize) -> Option<Duration> {
    if attempt >= DEPLOY_STREAM_MAX_ATTEMPTS {
        return None;
    }
    DEPLOY_STREAM_RETRY_BACKOFF_MS
        .get(attempt.saturating_sub(1))
        .copied()
        .map(Duration::from_millis)
}

fn request_stream_max_attempts(policy: RequestStreamRetryPolicy) -> usize {
    match policy {
        RequestStreamRetryPolicy::Standard => DEPLOY_STREAM_MAX_ATTEMPTS,
        RequestStreamRetryPolicy::CommandStartNoRetry => 1,
    }
}

fn request_stream_retry_backoff(
    policy: RequestStreamRetryPolicy,
    attempt: usize,
) -> Option<Duration> {
    match policy {
        RequestStreamRetryPolicy::Standard => deploy_stream_retry_backoff(attempt),
        RequestStreamRetryPolicy::CommandStartNoRetry => None,
    }
}

fn request_stream_read_timeout(
    policy: RequestStreamRetryPolicy,
    timeout: Duration,
) -> Option<Duration> {
    match policy {
        RequestStreamRetryPolicy::Standard => Some(timeout),
        RequestStreamRetryPolicy::CommandStartNoRetry => None,
    }
}

fn format_request_stream_retry_log_message(
    attempt: usize,
    total_attempts: usize,
    backoff: Duration,
    reason: &str,
) -> String {
    format!(
        "request stream attempt {attempt}/{total_attempts} failed, retrying in {}ms (reason={reason})",
        backoff.as_millis()
    )
}

fn format_request_stream_failure_summary(
    attempt: usize,
    first_failure: Option<&str>,
    last_failure: &str,
) -> String {
    match first_failure {
        Some(first) if first != last_failure => format!(
            "request stream failed after {attempt} attempts (first_failure={first}; last_failure={last_failure})"
        ),
        _ => format!("request stream failed on attempt {attempt} (reason={last_failure})"),
    }
}

pub(crate) fn resolve_deploy_stream_timeout() -> Duration {
    let value = std::env::var("IMAGO_DEPLOY_STREAM_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_DEPLOY_STREAM_TIMEOUT_SECS);
    Duration::from_secs(value)
}

fn upload_ranges_for_prepare(
    status: ArtifactStatus,
    missing_ranges: &[ByteRange],
    artifact_size: u64,
) -> anyhow::Result<Vec<ByteRange>> {
    match status {
        ArtifactStatus::Complete => Ok(Vec::new()),
        ArtifactStatus::Missing => Ok(vec![ByteRange {
            offset: 0,
            length: artifact_size,
        }]),
        ArtifactStatus::Partial => {
            if missing_ranges.is_empty() {
                return Err(anyhow!(
                    "server reported artifact_status=partial but missing_ranges is empty"
                ));
            }
            Ok(missing_ranges.to_vec())
        }
    }
}

fn build_upload_chunk_plan(
    ranges: &[ByteRange],
    artifact_size: u64,
    chunk_size: usize,
) -> anyhow::Result<Vec<(u64, usize)>> {
    if chunk_size == 0 {
        return Err(anyhow!("chunk_size must be greater than 0"));
    }

    let chunk_size_u64 = u64::try_from(chunk_size).context("chunk_size conversion failed")?;
    let mut chunks = Vec::new();
    for range in ranges {
        if range.length == 0 {
            return Err(anyhow!("missing range length must be greater than 0"));
        }
        let range_end = range
            .offset
            .checked_add(range.length)
            .ok_or_else(|| anyhow!("missing range overflow: offset+length"))?;
        if range_end > artifact_size {
            return Err(anyhow!(
                "missing range is outside artifact size: end={} artifact_size={}",
                range_end,
                artifact_size
            ));
        }

        let mut cursor = range.offset;
        while cursor < range_end {
            let remaining = range_end - cursor;
            let chunk_len_u64 = remaining.min(chunk_size_u64);
            let chunk_len =
                usize::try_from(chunk_len_u64).context("chunk length conversion failed")?;
            chunks.push((cursor, chunk_len));
            cursor = cursor.saturating_add(chunk_len_u64);
        }
    }
    Ok(chunks)
}

async fn push_artifact_ranges(
    context: UploadRequestContext<'_>,
    artifact_path: &Path,
    artifact_size: u64,
    ranges: &[ByteRange],
    limits: UploadLimits,
) -> anyhow::Result<()> {
    let chunk_plan = build_upload_chunk_plan(ranges, artifact_size, limits.chunk_size)?;

    let mut file = tokio::fs::File::open(artifact_path)
        .await
        .with_context(|| {
            format!(
                "failed to open artifact bundle: {}",
                artifact_path.display()
            )
        })?;

    let mut uploads = JoinSet::new();
    let deploy_id = Arc::<str>::from(context.deploy_id.to_string());
    let upload_token = Arc::<str>::from(context.upload_token.to_string());

    let max_inflight_chunks = 1usize;

    for (offset, chunk_len) in chunk_plan {
        while uploads.len() >= max_inflight_chunks {
            let completed = uploads
                .join_next()
                .await
                .ok_or_else(|| anyhow!("upload task set was unexpectedly empty"))?;
            let uploaded = completed.map_err(|err| anyhow!("upload task join failed: {err}"))??;
            ui::command_upload_inc("service.deploy", uploaded);
        }

        let mut chunk = vec![0u8; chunk_len];
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .with_context(|| {
                format!(
                    "failed to seek artifact bundle: {}",
                    artifact_path.display()
                )
            })?;
        file.read_exact(&mut chunk).await.with_context(|| {
            format!(
                "failed to read artifact bundle chunk: {}",
                artifact_path.display()
            )
        })?;
        let task_session = context.session.clone();
        let task_deploy_id = deploy_id.clone();
        let task_upload_token = upload_token.clone();
        let task_stream_timeout = context.stream_timeout;
        let uploaded_len = u64::try_from(chunk_len).context("chunk length conversion failed")?;
        uploads.spawn(async move {
            push_single_artifact_chunk(
                task_session,
                context.correlation_id,
                task_deploy_id,
                task_upload_token,
                offset,
                chunk,
                task_stream_timeout,
            )
            .await?;
            Ok::<u64, anyhow::Error>(uploaded_len)
        });
    }

    while let Some(completed) = uploads.join_next().await {
        let uploaded = completed.map_err(|err| anyhow!("upload task join failed: {err}"))??;
        ui::command_upload_inc("service.deploy", uploaded);
    }

    Ok(())
}

async fn push_single_artifact_chunk(
    session: ConnectedTargetSession,
    correlation_id: Uuid,
    deploy_id: Arc<str>,
    upload_token: Arc<str>,
    offset: u64,
    chunk: Vec<u8>,
    stream_timeout: Duration,
) -> anyhow::Result<()> {
    let chunk_hash = hex::encode(Sha256::digest(&chunk));
    let length = u64::try_from(chunk.len()).context("chunk length conversion failed")?;

    let framed = encode_artifact_push_request_frame(
        Uuid::new_v4(),
        correlation_id,
        ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: deploy_id.as_ref().to_string(),
                offset,
                length,
                chunk_sha256: chunk_hash,
                upload_token: upload_token.as_ref().to_string(),
            },
            chunk,
        },
    )?;
    let mut responses = request_events_with_retry_policy_framed(
        &session,
        &framed,
        stream_timeout,
        RequestStreamRetryPolicy::Standard,
    )
    .await?;
    let response = responses
        .drain(..)
        .next()
        .ok_or_else(|| anyhow!("empty response stream"))?;
    let _ack: imago_protocol::ArtifactPushAck = response_payload(response)?;
    Ok(())
}

fn encode_artifact_push_request_frame(
    request_id: Uuid,
    correlation_id: Uuid,
    payload: ArtifactPushRequest,
) -> anyhow::Result<Vec<u8>> {
    let envelope = ProtocolEnvelope {
        message_type: MessageType::ArtifactPush,
        request_id,
        correlation_id,
        payload,
        error: None,
    };
    let payload = to_cbor(&envelope)?;
    Ok(encode_frame(&payload))
}

pub(crate) fn response_payload<T: serde::de::DeserializeOwned>(
    response: Envelope,
) -> anyhow::Result<T> {
    if let Some(error) = response.error {
        return Err(ServerResponseError { error }.into());
    }
    serde_json::from_value(response.payload)
        .map_err(|e| anyhow!("response payload decode failed: {e}"))
}

#[derive(Debug, Deserialize)]
struct ResponseEnvelopeHeader {
    #[serde(rename = "type")]
    message_type: MessageType,
}

fn decode_response_envelope(frame: &[u8]) -> anyhow::Result<Envelope> {
    let header: ResponseEnvelopeHeader =
        from_cbor(frame).context("failed to decode streamed response header")?;
    match header.message_type {
        MessageType::LogsChunk => normalize_typed_response_payload::<LogChunk>(frame, "logs.chunk"),
        MessageType::LogsEnd => normalize_typed_response_payload::<LogEnd>(frame, "logs.end"),
        _ => from_cbor(frame)
            .with_context(|| format!("failed to decode {:?} frame", header.message_type)),
    }
}

fn normalize_typed_response_payload<T>(frame: &[u8], label: &str) -> anyhow::Result<Envelope>
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let envelope: ProtocolEnvelope<T> =
        from_cbor(frame).with_context(|| format!("failed to decode {label} frame"))?;
    Ok(Envelope {
        message_type: envelope.message_type,
        request_id: envelope.request_id,
        correlation_id: envelope.correlation_id,
        payload: serde_json::to_value(envelope.payload)
            .with_context(|| format!("failed to normalize {label} payload"))?,
        error: envelope.error,
    })
}

fn build_idempotency_key(
    name: &str,
    app_type: &str,
    target: &BTreeMap<String, String>,
    policy: &BTreeMap<String, String>,
    artifact_digest: &str,
    artifact_size: u64,
    manifest_digest: &str,
) -> String {
    let mut hasher = Sha256::new();
    update_canonical_field(&mut hasher, "name", name);
    update_canonical_field(&mut hasher, "app_type", app_type);
    update_canonical_field(&mut hasher, "artifact_digest", artifact_digest);
    update_canonical_field(&mut hasher, "artifact_size", &artifact_size.to_string());
    update_canonical_field(&mut hasher, "manifest_digest", manifest_digest);
    update_canonical_map(&mut hasher, "target", target);
    update_canonical_map(&mut hasher, "policy", policy);
    format!("deploy:{}", hex::encode(hasher.finalize()))
}

fn update_canonical_field(hasher: &mut Sha256, key: &str, value: &str) {
    hasher.update(key.as_bytes());
    hasher.update(b"\0");
    hasher.update(value.len().to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(value.as_bytes());
    hasher.update(b"\0");
}

fn update_canonical_map(hasher: &mut Sha256, key: &str, map: &BTreeMap<String, String>) {
    hasher.update(key.as_bytes());
    hasher.update(b"\0");
    for (entry_key, entry_value) in map {
        update_canonical_field(hasher, entry_key, entry_value);
    }
    hasher.update(b"\0");
}

fn normalize_target_for_protocol(target: &build::DeployTargetConfig) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    map.insert("remote".to_string(), target.remote.clone());
    map
}

fn build_artifact_bundle_file(
    manifest: &Manifest,
    manifest_source: &Path,
    project_root: &Path,
    dependency_component_sources: &BTreeMap<String, PathBuf>,
) -> anyhow::Result<TempArtifactBundle> {
    let bundle_path = std::env::temp_dir().join(format!("imago-artifact-{}.tar", Uuid::new_v4()));
    let bundle_file = std::fs::File::create(&bundle_path).with_context(|| {
        format!(
            "failed to create artifact bundle file: {}",
            bundle_path.display()
        )
    })?;

    let mut builder = tar::Builder::new(bundle_file);
    add_file_to_tar(
        &mut builder,
        project_root.join(manifest_source),
        "manifest.json",
    )?;
    let manifest_base_dir = manifest_source.parent().unwrap_or_else(|| Path::new(""));
    let normalized_main = normalize_bundle_entry_path(&manifest.main, "manifest.main")?;
    let main_entry = normalized_tar_entry_name(&normalized_main);
    add_file_to_tar(
        &mut builder,
        project_root.join(manifest_base_dir).join(&normalized_main),
        &main_entry,
    )?;
    for asset in &manifest.assets {
        let normalized_asset = normalize_bundle_entry_path(&asset.path, "assets[].path")?;
        let asset_entry = normalized_tar_entry_name(&normalized_asset);
        add_file_to_tar(
            &mut builder,
            project_root.join(&normalized_asset),
            &asset_entry,
        )?;
    }
    for (index, dependency) in manifest.dependencies.iter().enumerate() {
        if dependency.kind != build::ManifestDependencyKind::Wasm {
            continue;
        }
        let component = dependency.component.as_ref().ok_or_else(|| {
            anyhow!("dependencies[{index}].component is required when kind=\"wasm\"")
        })?;
        let normalized_component = normalize_bundle_entry_path(
            &component.path,
            &format!("dependencies[{index}].component.path"),
        )?;
        let component_entry = normalized_tar_entry_name(&normalized_component);
        let source_path = dependency_component_sources
            .get(&dependency.name)
            .cloned()
            .unwrap_or_else(|| project_root.join(&normalized_component));
        add_file_to_tar(&mut builder, source_path, &component_entry)?;
    }
    builder.finish()?;

    Ok(TempArtifactBundle::new(bundle_path))
}

#[cfg(test)]
async fn resolve_dependency_component_sources(
    project_root: &Path,
    manifest: &Manifest,
) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    let dependency_resolver = StandardDependencyResolver;
    dependency_resolver.resolve_dependency_component_sources(project_root, &manifest.dependencies)
}

fn normalize_bundle_entry_path(raw: &str, field_name: &str) -> anyhow::Result<PathBuf> {
    if raw.is_empty() {
        return Err(anyhow!("{field_name} must not be empty"));
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(anyhow!("{field_name} must be a relative path: {raw}"));
    }
    if raw.contains('\\') {
        return Err(anyhow!(
            "{field_name} must not contain backslash separators: {raw}"
        ));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(anyhow!("{field_name} must not be windows-prefixed: {raw}"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir => {
                return Err(anyhow!(
                    "{field_name} must not contain path traversal: {raw}"
                ));
            }
            _ => {
                return Err(anyhow!(
                    "{field_name} contains unsupported path component: {raw}"
                ));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("{field_name} is invalid: {raw}"));
    }

    Ok(normalized)
}

fn normalized_tar_entry_name(path: &Path) -> String {
    path.iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn compute_file_sha256_and_size(path: &Path) -> anyhow::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to open artifact bundle: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;

    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read artifact bundle: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }

    Ok((hex::encode(hasher.finalize()), total))
}

fn add_file_to_tar<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    source: PathBuf,
    entry_name: &str,
) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(&source)
        .with_context(|| format!("failed to open file for artifact: {}", source.display()))?;
    builder
        .append_file(entry_name, &mut file)
        .with_context(|| format!("failed to append tar entry: {entry_name}"))?;
    Ok(())
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_frames(value: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut offset = 0usize;

    while offset < value.len() {
        if value.len() - offset < 4 {
            return Err(anyhow!("truncated frame header"));
        }

        let len = u32::from_be_bytes(
            value[offset..offset + 4]
                .try_into()
                .map_err(|_| anyhow!("invalid frame header"))?,
        ) as usize;
        offset += 4;

        if value.len() - offset < len {
            return Err(anyhow!("truncated frame payload"));
        }

        out.push(value[offset..offset + len].to_vec());
        offset += len;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::dependency_cache;
    use crate::commands::deploy::network::AdminTransport as _;
    use crate::commands::error_diagnostics::format_command_error;
    use crate::lockfile::{
        ComponentExpectation, DependencyExpectation, IMAGO_LOCK_VERSION, ImagoLock,
        ImagoLockResolved, ImagoLockResolvedDependency, LockCapabilityPolicy, LockDependencyKind,
        LockSourceKind, build_requested_snapshot, compute_dependency_request_id,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    #[cfg(unix)]
    use tokio::{io::AsyncWriteExt, net::UnixListener};

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-deploy-tests-{test_name}-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be created");
        }
        fs::write(path, bytes).expect("file should be written");
    }

    #[cfg(unix)]
    fn new_temp_socket_path(test_name: &str) -> PathBuf {
        let _ = test_name;
        PathBuf::from(format!("/tmp/imago-{}.sock", Uuid::new_v4().simple()))
    }

    struct CountingTransport {
        close_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl network::AdminTransport for CountingTransport {
        fn close(&self) {
            self.close_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn request_response_bytes(
            &self,
            _framed: &[u8],
            _open_write_timeout: Duration,
            _read_timeout: Option<Duration>,
        ) -> anyhow::Result<Vec<u8>> {
            unreachable!("counting transport does not support requests")
        }

        async fn stream_response_frames(
            &self,
            _framed: &[u8],
            _open_write_timeout: Duration,
            _read_idle_timeout: Option<Duration>,
            _follow: bool,
            _on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
        ) -> anyhow::Result<StreamRequestTermination> {
            unreachable!("counting transport does not support streams")
        }
    }

    fn counting_session(close_count: Arc<AtomicUsize>) -> ConnectedTargetSession {
        ConnectedTargetSession {
            transport: Arc::new(CountingTransport { close_count }),
            authority: "authority".to_string(),
            resolved_addr: "127.0.0.1:0".to_string(),
            configured_host: "localhost".to_string(),
            remote_input: "ssh://localhost".to_string(),
        }
    }

    fn write_imago_lock(root: &Path, lock: &ImagoLock) {
        let body = toml::to_string_pretty(lock).expect("lock should serialize");
        write_file(&root.join("imago.lock"), body.as_bytes());
    }

    fn write_project_with_single_wasm_dependency(root: &Path) {
        write_file(
            &root.join("imago.toml"),
            br#"
    name = "svc"
    main = "build/app.wasm"
    type = "cli"
    
    [[dependencies]]
    version = "0.1.0"
    kind = "wasm"
    path = "registry/example"
    
    [dependencies.component]
    path = "registry/example-component.wasm"

    [target.default]
    remote = "ssh://localhost?socket=/run/imago/imagod.sock"
    "#,
        );
        write_file(
            &root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\n",
        );
        write_file(
            &root.join("wit/deps/path-source-0-0.1.0/package.wit"),
            b"package test:example@0.1.0;\n",
        );
    }

    fn lock_with_single_wasm_dependency(root: &Path, component_sha: &str) -> ImagoLock {
        let wit_path = "wit/deps/path-source-0-0.1.0";
        let wit_tree_digest = build::compute_path_digest_hex(&root.join(wit_path))
            .expect("wit digest should compute");
        let expectation = DependencyExpectation {
            name: "path-source-0".to_string(),
            kind: LockDependencyKind::Wasm,
            version: "0.1.0".to_string(),
            source_kind: LockSourceKind::Path,
            source: "registry/example".to_string(),
            registry: None,
            sha256: None,
            requires: vec![],
            capabilities: LockCapabilityPolicy::default(),
            component: Some(ComponentExpectation {
                source_kind: LockSourceKind::Path,
                source: "registry/example-component.wasm".to_string(),
                registry: None,
                sha256: None,
            }),
        };
        let request_id = compute_dependency_request_id(&expectation);
        let requested = build_requested_snapshot(&[expectation], &[], &[], None)
            .expect("requested snapshot should be built");
        ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![ImagoLockResolvedDependency {
                    request_id,
                    resolved_name: "test:example".to_string(),
                    resolved_version: "0.1.0".to_string(),
                    wit_path: wit_path.to_string(),
                    wit_tree_digest,
                    component_source: Some("registry/example-component.wasm".to_string()),
                    component_registry: None,
                    component_sha256: Some(component_sha.to_string()),
                    requires_request_ids: vec![],
                }],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        }
    }

    fn sample_manifest_with_wasm_dependency(name: &str, sha256: &str) -> Manifest {
        Manifest {
            name: "svc".to_string(),
            main: "app.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![build::ManifestDependency {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                kind: build::ManifestDependencyKind::Wasm,
                wit: "file://registry/example.wit".to_string(),
                requires: vec![],
                component: Some(build::ManifestDependencyComponent {
                    path: format!("plugins/components/{sha256}.wasm"),
                    sha256: sha256.to_string(),
                }),
                capabilities: build::ManifestCapabilityPolicy::default(),
            }],
        }
    }

    #[test]
    fn connected_session_close_guard_closes_session_on_drop() {
        let close_count = Arc::new(AtomicUsize::new(0));
        let session = counting_session(close_count.clone());
        {
            let _guard = ConnectedSessionCloseGuard::new(&session, b"done");
        }

        assert_eq!(close_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn connected_session_close_guard_can_be_disarmed() {
        let close_count = Arc::new(AtomicUsize::new(0));
        let session = counting_session(close_count.clone());
        {
            let mut guard = ConnectedSessionCloseGuard::new(&session, b"done");
            guard.disarm();
        }

        assert_eq!(close_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn disarmed_close_guard_allows_manual_close_without_double_close() {
        let close_count = Arc::new(AtomicUsize::new(0));
        let session = counting_session(close_count.clone());
        {
            let mut guard = ConnectedSessionCloseGuard::new(&session, b"done");
            guard.disarm();
            session.close(0, b"manual");
        }

        assert_eq!(close_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn response_payload_decodes_logs_chunk_byte_string_fields() {
        let chunk = imago_protocol::LogChunk {
            request_id: Uuid::new_v4(),
            seq: 3,
            name: "svc".to_string(),
            stream_kind: imago_protocol::LogStreamKind::Stdout,
            bytes: b"hello".to_vec(),
            is_last: false,
            timestamp_unix_ms: Some(1234),
        };
        let encoded = to_cbor(&ProtocolEnvelope::new(
            MessageType::LogsChunk,
            chunk.request_id,
            Uuid::new_v4(),
            chunk.clone(),
        ))
        .expect("typed envelope should encode");
        let response =
            decode_response_envelope(&encoded).expect("response envelope should normalize");

        let decoded: imago_protocol::LogChunk =
            response_payload(response).expect("response payload should decode");

        assert_eq!(decoded, chunk);
    }

    #[test]
    fn response_payload_decodes_logs_end_payload() {
        let end = imago_protocol::LogEnd {
            request_id: Uuid::new_v4(),
            seq: 4,
            error: None,
        };
        let encoded = to_cbor(&ProtocolEnvelope::new(
            MessageType::LogsEnd,
            end.request_id,
            Uuid::new_v4(),
            end.clone(),
        ))
        .expect("typed envelope should encode");
        let response =
            decode_response_envelope(&encoded).expect("response envelope should normalize");

        let decoded: imago_protocol::LogEnd =
            response_payload(response).expect("response payload should decode");

        assert_eq!(decoded, end);
    }

    #[test]
    fn ssh_proxy_command_args_enable_batch_mode_and_forward_socket_override() {
        let remote = build::SshTargetRemote {
            user: Some("root".to_string()),
            host: "edge.example.com".to_string(),
            port: Some(2222),
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };

        let args = ssh_proxy_command_args(&remote);

        assert_eq!(args[0], "-T");
        assert_eq!(args[1], "-o");
        assert_eq!(args[2], "BatchMode=yes");
        assert!(args.windows(2).any(|pair| pair == ["-p", "2222"]));
        assert!(args.iter().any(|arg| arg == "root@edge.example.com"));
        assert!(args.iter().any(|arg| arg == "proxy-stdio"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--socket", "/tmp/imagod.sock"])
        );
    }

    #[test]
    fn required_local_proxy_socket_path_accepts_loopback_socket() {
        let remote = build::SshTargetRemote {
            user: None,
            host: "localhost".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };

        assert_eq!(
            required_local_proxy_socket_path(&remote).expect("loopback socket should be accepted"),
            "/tmp/imagod.sock"
        );
    }

    #[test]
    fn required_local_proxy_socket_path_rejects_missing_socket() {
        let remote = build::SshTargetRemote {
            user: None,
            host: "localhost".to_string(),
            port: None,
            socket_path: None,
        };

        let err = required_local_proxy_socket_path(&remote)
            .expect_err("missing socket should be rejected");
        assert!(err.to_string().contains("requires ?socket=/abs/path"));
    }

    #[test]
    fn required_local_proxy_socket_path_rejects_non_loopback_host() {
        let remote = build::SshTargetRemote {
            user: None,
            host: "edge.example.com".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };

        let err = required_local_proxy_socket_path(&remote)
            .expect_err("non-loopback host should be rejected");
        assert!(
            err.to_string()
                .contains("only supports loopback ssh targets")
        );
    }

    #[test]
    fn default_target_transport_kind_uses_direct_socket_only_for_loopback_without_user_or_port() {
        let loopback = build::SshTargetRemote {
            user: None,
            host: "localhost".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };
        let ipv4 = build::SshTargetRemote {
            user: None,
            host: "127.0.0.1".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };
        let ipv6 = build::SshTargetRemote {
            user: None,
            host: "::1".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };
        let bracketed_ipv6 = build::SshTargetRemote {
            user: None,
            host: "[::1]".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };
        let with_user = build::SshTargetRemote {
            user: Some("root".to_string()),
            host: "localhost".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };
        let with_port = build::SshTargetRemote {
            user: None,
            host: "localhost".to_string(),
            port: Some(2222),
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };
        let missing_socket = build::SshTargetRemote {
            user: None,
            host: "localhost".to_string(),
            port: None,
            socket_path: None,
        };
        let remote_host = build::SshTargetRemote {
            user: None,
            host: "edge.example.com".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };

        #[cfg(unix)]
        {
            assert_eq!(
                default_target_transport_kind(&loopback),
                DefaultTargetTransportKind::DirectSocket
            );
            assert_eq!(
                default_target_transport_kind(&ipv4),
                DefaultTargetTransportKind::DirectSocket
            );
            assert_eq!(
                default_target_transport_kind(&ipv6),
                DefaultTargetTransportKind::DirectSocket
            );
            assert_eq!(
                default_target_transport_kind(&bracketed_ipv6),
                DefaultTargetTransportKind::DirectSocket
            );
        }

        #[cfg(not(unix))]
        {
            assert_eq!(
                default_target_transport_kind(&loopback),
                DefaultTargetTransportKind::Ssh
            );
            assert_eq!(
                default_target_transport_kind(&ipv4),
                DefaultTargetTransportKind::Ssh
            );
            assert_eq!(
                default_target_transport_kind(&ipv6),
                DefaultTargetTransportKind::Ssh
            );
            assert_eq!(
                default_target_transport_kind(&bracketed_ipv6),
                DefaultTargetTransportKind::Ssh
            );
        }

        assert_eq!(
            default_target_transport_kind(&with_user),
            DefaultTargetTransportKind::Ssh
        );
        assert_eq!(
            default_target_transport_kind(&with_port),
            DefaultTargetTransportKind::Ssh
        );
        assert_eq!(
            default_target_transport_kind(&missing_socket),
            DefaultTargetTransportKind::Ssh
        );
        assert_eq!(
            default_target_transport_kind(&remote_host),
            DefaultTargetTransportKind::Ssh
        );
    }

    #[cfg(unix)]
    #[test]
    fn required_direct_socket_path_rejects_user_or_port_overrides() {
        let with_user = build::SshTargetRemote {
            user: Some("root".to_string()),
            host: "localhost".to_string(),
            port: None,
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };
        let with_port = build::SshTargetRemote {
            user: None,
            host: "localhost".to_string(),
            port: Some(2222),
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };

        let user_err = required_direct_socket_path(&with_user)
            .expect_err("direct socket connector must reject user override");
        assert!(user_err.to_string().contains("without user/port overrides"));

        let port_err = required_direct_socket_path(&with_port)
            .expect_err("direct socket connector must reject port override");
        assert!(port_err.to_string().contains("without user/port overrides"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_socket_request_response_bytes_uses_framed_protocol() {
        let socket_path = new_temp_socket_path("direct-request-response");
        let listener =
            UnixListener::bind(&socket_path).expect("test unix listener should bind successfully");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("server should accept");
            let request = read_stdio_response_message_with_label(&mut stream, "test server")
                .await
                .expect("server should read framed request");
            stream
                .write_all(&encode_frame(b"reply-frame"))
                .await
                .expect("server should write response frame");
            stream.flush().await.expect("server should flush response");
            request
        });

        let session = DirectSocketTargetSession {
            socket_path: socket_path.display().to_string(),
        };
        let request = encode_frame(b"request-frame");
        let response = session
            .request_response_bytes(
                &request,
                Duration::from_secs(1),
                Some(Duration::from_secs(1)),
            )
            .await
            .expect("direct socket request should succeed");

        assert_eq!(
            server.await.expect("server task should complete"),
            request,
            "server should receive the exact framed request"
        );
        assert_eq!(response, encode_frame(b"reply-frame"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_socket_stream_response_frames_reads_multiple_frames() {
        let socket_path = new_temp_socket_path("direct-stream-frames");
        let listener =
            UnixListener::bind(&socket_path).expect("test unix listener should bind successfully");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("server should accept");
            let request = read_stdio_response_message_with_label(&mut stream, "test server")
                .await
                .expect("server should read framed request");
            for frame in [b"frame-a".as_slice(), b"frame-b".as_slice()] {
                stream
                    .write_all(&encode_frame(frame))
                    .await
                    .expect("server should write response frame");
            }
            stream.flush().await.expect("server should flush response");
            request
        });

        let session = DirectSocketTargetSession {
            socket_path: socket_path.display().to_string(),
        };
        let request = encode_frame(b"stream-request");
        let mut frames = Vec::new();
        let termination = session
            .stream_response_frames(
                &request,
                Duration::from_secs(1),
                Some(Duration::from_secs(1)),
                true,
                &mut |frame| {
                    frames.push(frame);
                    Ok(false)
                },
            )
            .await
            .expect("direct socket stream should succeed");

        assert_eq!(
            server.await.expect("server task should complete"),
            request,
            "server should receive the exact framed request"
        );
        assert_eq!(termination, StreamRequestTermination::Completed);
        assert_eq!(frames, vec![b"frame-a".to_vec(), b"frame-b".to_vec()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_socket_request_response_rejects_truncated_header() {
        let socket_path = new_temp_socket_path("direct-truncated-header");
        let listener =
            UnixListener::bind(&socket_path).expect("test unix listener should bind successfully");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("server should accept");
            let _request = read_stdio_response_message_with_label(&mut stream, "test server")
                .await
                .expect("server should read framed request");
            stream
                .write_all(&[0x00, 0x00])
                .await
                .expect("server should write partial header");
            stream
                .flush()
                .await
                .expect("server should flush partial header");
        });

        let session = DirectSocketTargetSession {
            socket_path: socket_path.display().to_string(),
        };
        let request = encode_frame(b"request-frame");
        let err = session
            .request_response_bytes(
                &request,
                Duration::from_secs(1),
                Some(Duration::from_secs(1)),
            )
            .await
            .expect_err("truncated header must fail");

        server.await.expect("server task should complete");
        assert!(
            err.to_string()
                .contains("closed in the middle of a response")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_socket_request_response_rejects_truncated_payload() {
        let socket_path = new_temp_socket_path("direct-truncated-payload");
        let listener =
            UnixListener::bind(&socket_path).expect("test unix listener should bind successfully");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("server should accept");
            let _request = read_stdio_response_message_with_label(&mut stream, "test server")
                .await
                .expect("server should read framed request");
            stream
                .write_all(&4u32.to_be_bytes())
                .await
                .expect("server should write frame header");
            stream
                .write_all(&[0x01, 0x02])
                .await
                .expect("server should write partial payload");
            stream
                .flush()
                .await
                .expect("server should flush partial payload");
        });

        let session = DirectSocketTargetSession {
            socket_path: socket_path.display().to_string(),
        };
        let request = encode_frame(b"request-frame");
        let err = session
            .request_response_bytes(
                &request,
                Duration::from_secs(1),
                Some(Duration::from_secs(1)),
            )
            .await
            .expect_err("truncated payload must fail");

        server.await.expect("server task should complete");
        assert!(
            err.to_string()
                .contains("closed in the middle of a response")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_socket_request_response_reports_daemon_not_running_when_socket_missing() {
        let socket_path = new_temp_socket_path("direct-missing-socket");
        let session = DirectSocketTargetSession {
            socket_path: socket_path.display().to_string(),
        };
        let request = encode_frame(b"request-frame");

        let err = session
            .request_response_bytes(
                &request,
                Duration::from_secs(1),
                Some(Duration::from_secs(1)),
            )
            .await
            .expect_err("missing socket must fail");

        assert!(
            err.to_string()
                .contains("Cannot connect to the imagod daemon at")
        );
        assert!(
            err.to_string()
                .contains(&format!("unix://{}", socket_path.display()))
        );
        assert!(err.to_string().contains("Is the imagod daemon running?"));
    }

    #[test]
    fn recover_ssh_read_result_resets_only_on_error() {
        let reset_calls = std::cell::Cell::new(0usize);

        let ok = recover_ssh_read_result(Ok::<_, anyhow::Error>(7usize), || {
            reset_calls.set(reset_calls.get() + 1);
            Ok(())
        })
        .expect("ok result should pass through");
        assert_eq!(ok, 7);
        assert_eq!(reset_calls.get(), 0);

        let err = recover_ssh_read_result(
            Err::<usize, anyhow::Error>(anyhow!("ssh read failed")),
            || {
                reset_calls.set(reset_calls.get() + 1);
                Ok(())
            },
        )
        .expect_err("error result should remain an error");
        assert!(err.to_string().contains("ssh read failed"));
        assert_eq!(reset_calls.get(), 1);
    }

    #[test]
    fn local_proxy_request_failure_includes_transport_stderr_in_diagnostics() {
        let err = annotate_local_proxy_transport_error(
            anyhow!("local proxy transport read timed out after 100 ms"),
            Some("proxy socket unavailable".to_string()),
        );

        let diagnostic = format_command_error("service.deploy", &err);
        assert!(
            diagnostic.contains("local proxy transport stderr: proxy socket unavailable"),
            "unexpected diagnostic: {diagnostic}"
        );
    }

    #[tokio::test]
    async fn returns_non_zero_when_load_config_fails_before_build() {
        let root =
            std::env::temp_dir().join(format!("imago-cli-deploy-run-fail-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("temp dir should be created");

        let result = run_with_project_root(
            DeployArgs {
                target: None,
                detach: false,
            },
            &root,
        )
        .await;

        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.expect("stderr should be present");
        assert!(stderr.contains("failed to load target configuration"));
        assert!(stderr.contains("caused by:"));
        assert!(stderr.contains("hint:"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resolve_dependency_component_sources_reads_component_from_dependency_cache() {
        let root = new_temp_dir("dependency-cache-hit");
        let component_bytes = b"\0asmcached-component";
        let component_sha = hex::encode(Sha256::digest(component_bytes));
        write_project_with_single_wasm_dependency(&root);

        write_imago_lock(
            &root,
            &lock_with_single_wasm_dependency(&root, &component_sha),
        );

        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "path-source-0".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "registry/example".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: "wit/deps/path-source-0-0.1.0".to_string(),
            wit_digest: build::compute_path_digest_hex(&root.join("wit/deps/path-source-0-0.1.0"))
                .expect("wit digest should compute"),
            wit_source_fingerprint: None,
            component_source: Some("registry/example-component.wasm".to_string()),
            component_registry: None,
            component_sha256: Some(component_sha.clone()),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        };
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache metadata should be written");
        write_file(
            &dependency_cache::cache_component_path(&root, "path-source-0", &component_sha),
            component_bytes,
        );

        let manifest = sample_manifest_with_wasm_dependency("test:example", &component_sha);
        let sources = resolve_dependency_component_sources(&root, &manifest)
            .await
            .expect("dependency component should resolve from cache");
        let resolved = sources
            .get("test:example")
            .expect("resolved source should exist");
        assert_eq!(
            resolved,
            &dependency_cache::cache_component_path(&root, "path-source-0", &component_sha)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resolve_dependency_component_sources_uses_project_dependency_id_for_cache_lookup() {
        let root = new_temp_dir("dependency-cache-id-lookup");
        let component_bytes = b"\0asmcached-component";
        let component_sha = hex::encode(Sha256::digest(component_bytes));
        write_project_with_single_wasm_dependency(&root);

        write_imago_lock(
            &root,
            &lock_with_single_wasm_dependency(&root, &component_sha),
        );

        let cache_entry = dependency_cache::DependencyCacheEntry {
            name: "path-source-0".to_string(),
            resolved_package_name: None,
            version: "0.1.0".to_string(),
            kind: "wasm".to_string(),
            wit_source: "registry/example".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: "wit/deps/path-source-0-0.1.0".to_string(),
            wit_digest: build::compute_path_digest_hex(&root.join("wit/deps/path-source-0-0.1.0"))
                .expect("wit digest should compute"),
            wit_source_fingerprint: None,
            component_source: Some("registry/example-component.wasm".to_string()),
            component_registry: None,
            component_sha256: Some(component_sha.clone()),
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        };
        dependency_cache::save_entry(&root, &cache_entry)
            .expect("dependency cache metadata should be written");
        write_file(
            &dependency_cache::cache_component_path(&root, "path-source-0", &component_sha),
            component_bytes,
        );

        let manifest = sample_manifest_with_wasm_dependency("test:example", &component_sha);
        let sources = resolve_dependency_component_sources(&root, &manifest)
            .await
            .expect("dependency component should resolve from cache");
        assert!(sources.contains_key("test:example"));
        assert!(!sources.contains_key("path-source-0"));
        assert_eq!(
            sources.get("test:example"),
            Some(&dependency_cache::cache_component_path(
                &root,
                "path-source-0",
                &component_sha
            ))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resolve_dependency_component_sources_fails_when_manifest_dependency_name_is_not_resolved()
     {
        let root = new_temp_dir("dependency-name-unresolved");
        let component_sha = "0".repeat(64);
        write_project_with_single_wasm_dependency(&root);

        write_imago_lock(
            &root,
            &lock_with_single_wasm_dependency(&root, &component_sha),
        );

        let manifest = sample_manifest_with_wasm_dependency("test:unknown", &component_sha);
        let err = resolve_dependency_component_sources(&root, &manifest)
            .await
            .expect_err("unknown manifest dependency name must fail");
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains("dependency 'test:unknown' is not resolved in imago.lock"),
            "unexpected error: {err:#}"
        );
        assert!(
            err_chain.contains("imago deps sync"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resolve_dependency_component_sources_fails_when_dependency_cache_is_missing() {
        let root = new_temp_dir("dependency-cache-miss");
        let component_bytes = b"\0asmcached-component";
        let component_sha = hex::encode(Sha256::digest(component_bytes));
        write_project_with_single_wasm_dependency(&root);

        write_imago_lock(
            &root,
            &lock_with_single_wasm_dependency(&root, &component_sha),
        );

        let manifest = sample_manifest_with_wasm_dependency("test:example", &component_sha);
        let err = resolve_dependency_component_sources(&root, &manifest)
            .await
            .expect_err("missing dependency cache must fail");
        let err_chain = format!("{err:#}");
        assert!(
            err_chain.contains(".imago/deps"),
            "unexpected error: {err:#}"
        );
        assert!(
            err_chain.contains("imago deps sync"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn command_start_envelope_uses_same_request_id_for_header_and_payload() {
        let request_id = Uuid::new_v4();
        let envelope = build_command_start_envelope(
            Uuid::new_v4(),
            request_id,
            CommandType::Deploy,
            CommandPayload::Deploy(DeployCommandPayload {
                deploy_id: "deploy-1".to_string(),
                expected_current_release: "any".to_string(),
                restart_policy: "never".to_string(),
                auto_rollback: true,
            }),
        )
        .expect("envelope should be created");

        assert_eq!(envelope.request_id, request_id);
        let payload: CommandStartRequest =
            serde_json::from_value(envelope.payload).expect("payload should deserialize");
        assert_eq!(payload.request_id, request_id);
    }

    #[test]
    fn artifact_push_frame_encodes_chunk_as_compact_cbor_bytes() {
        let chunk = vec![u8::MAX; 8192];
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let typed_framed = encode_artifact_push_request_frame(
            request_id,
            correlation_id,
            ArtifactPushRequest {
                header: ArtifactPushChunkHeader {
                    deploy_id: "deploy-1".to_string(),
                    offset: 0,
                    length: u64::try_from(chunk.len()).expect("chunk length should fit u64"),
                    chunk_sha256: hex::encode(Sha256::digest(&chunk)),
                    upload_token: "token-1".to_string(),
                },
                chunk: chunk.clone(),
            },
        )
        .expect("typed push frame should encode");

        let legacy = request_envelope(
            MessageType::ArtifactPush,
            request_id,
            correlation_id,
            &ArtifactPushRequest {
                header: ArtifactPushChunkHeader {
                    deploy_id: "deploy-1".to_string(),
                    offset: 0,
                    length: u64::try_from(chunk.len()).expect("chunk length should fit u64"),
                    chunk_sha256: hex::encode(Sha256::digest(&chunk)),
                    upload_token: "token-1".to_string(),
                },
                chunk,
            },
        )
        .expect("legacy envelope should encode");
        let legacy_payload = to_cbor(&legacy).expect("legacy payload should encode");
        let legacy_framed = encode_frame(&legacy_payload);

        assert!(
            typed_framed.len() + 4096 < legacy_framed.len(),
            "typed frame should be much smaller than legacy JSON-value frame (typed={} legacy={})",
            typed_framed.len(),
            legacy_framed.len()
        );
    }

    #[test]
    fn stdio_response_message_growth_enforces_frame_and_total_limits() {
        let oversized_frame =
            ensure_stdio_response_frame_len_with_label(MAX_STREAM_BYTES + 1, "test transport")
                .expect_err("frame larger than limit should be rejected");
        assert!(
            oversized_frame
                .to_string()
                .contains("frame exceeds max size")
        );

        ensure_stdio_response_message_growth_with_label(0, MAX_STREAM_BYTES - 4, "test transport")
            .expect("a single framed response at the exact limit should be accepted");

        let oversized_total = ensure_stdio_response_message_growth_with_label(
            MAX_STREAM_BYTES - 4,
            1,
            "test transport",
        )
        .expect_err("cumulative framed bytes above the limit should be rejected");
        assert!(
            oversized_total
                .to_string()
                .contains("response exceeds max size")
        );
    }

    #[tokio::test]
    async fn read_stdio_response_message_rejects_oversized_frame_length() {
        let header = u32::try_from(MAX_STREAM_BYTES + 1)
            .expect("limit should fit in u32")
            .to_be_bytes();
        let mut reader = &header[..];

        let err = read_stdio_response_message_with_label(&mut reader, "test transport")
            .await
            .expect_err("oversized frame length should be rejected before allocation");
        assert!(err.to_string().contains("frame exceeds max size"));
    }

    #[tokio::test]
    async fn read_next_stdio_response_frame_rejects_oversized_frame_length() {
        let header = u32::try_from(MAX_STREAM_BYTES + 1)
            .expect("limit should fit in u32")
            .to_be_bytes();
        let mut reader = &header[..];

        let err = read_next_stdio_response_frame_with_label(&mut reader, "test transport")
            .await
            .expect_err("oversized frame length should be rejected before allocation");
        assert!(err.to_string().contains("frame exceeds max size"));
    }

    #[tokio::test]
    async fn read_next_stdio_response_frame_returns_none_for_zero_length_terminator() {
        let header = 0u32.to_be_bytes();
        let mut reader = &header[..];

        let frame = read_next_stdio_response_frame_with_label(&mut reader, "test transport")
            .await
            .expect("zero-length frame should terminate the stream");
        assert!(
            frame.is_none(),
            "zero-length frame should remain a terminator"
        );
    }

    #[test]
    fn parse_upload_limits_uses_hello_limits_values() {
        let response = HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod-test".to_string(),
            server_protocol_version: "0.1.0".to_string(),
            supported_protocol_version_range: ">=0.1.0,<0.2.0".to_string(),
            compatibility_announcement: None,
            features: vec![],
            limits: BTreeMap::from([
                ("chunk_size".to_string(), "2048".to_string()),
                ("max_inflight_chunks".to_string(), "4".to_string()),
                ("deploy_stream_timeout_secs".to_string(), "12".to_string()),
            ]),
        };

        let limits = parse_upload_limits(&response).expect("limits should parse");
        assert_eq!(
            limits,
            UploadLimits {
                chunk_size: 2048,
                max_inflight_chunks: 4,
                deploy_stream_timeout: Duration::from_secs(12),
            }
        );
    }

    #[test]
    fn parse_upload_limits_rejects_zero_values() {
        let response = HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod-test".to_string(),
            server_protocol_version: "0.1.0".to_string(),
            supported_protocol_version_range: ">=0.1.0,<0.2.0".to_string(),
            compatibility_announcement: None,
            features: vec![],
            limits: BTreeMap::from([("chunk_size".to_string(), "0".to_string())]),
        };

        let err = parse_upload_limits(&response).expect_err("zero chunk_size must fail");
        assert!(err.to_string().contains("chunk_size"));
    }

    #[test]
    fn command_stream_timeout_from_hello_limits_uses_valid_value() {
        let limits = BTreeMap::from([("deploy_stream_timeout_secs".to_string(), "45".to_string())]);
        let timeout = command_stream_timeout_from_hello_limits(&limits, Duration::from_secs(30));
        assert_eq!(timeout, Duration::from_secs(45));
    }

    #[test]
    fn command_stream_timeout_from_hello_limits_falls_back_when_missing() {
        let timeout =
            command_stream_timeout_from_hello_limits(&BTreeMap::new(), Duration::from_secs(30));
        assert_eq!(timeout, Duration::from_secs(30));
    }

    #[test]
    fn command_stream_timeout_from_hello_limits_falls_back_when_invalid_or_zero() {
        let invalid =
            BTreeMap::from([("deploy_stream_timeout_secs".to_string(), "abc".to_string())]);
        let invalid_timeout =
            command_stream_timeout_from_hello_limits(&invalid, Duration::from_secs(30));
        assert_eq!(invalid_timeout, Duration::from_secs(30));

        let zero = BTreeMap::from([("deploy_stream_timeout_secs".to_string(), "0".to_string())]);
        let zero_timeout = command_stream_timeout_from_hello_limits(&zero, Duration::from_secs(30));
        assert_eq!(zero_timeout, Duration::from_secs(30));
    }

    fn sample_structured_error(code: ErrorCode, retryable: bool) -> StructuredError {
        StructuredError {
            code,
            message: "error".to_string(),
            retryable,
            stage: "upload".to_string(),
            details: BTreeMap::new(),
        }
    }

    #[test]
    fn format_deploy_structured_error_includes_wasm_stdout_and_stderr_sections() {
        let mut error = sample_structured_error(ErrorCode::Internal, false);
        error
            .details
            .insert(DETAIL_WASM_STDOUT.to_string(), "stdout-line\n".to_string());
        error
            .details
            .insert(DETAIL_WASM_STDERR.to_string(), "stderr-line\n".to_string());

        let formatted = format_deploy_structured_error(&error);
        assert!(formatted.starts_with("error (Internal) at upload"));
        assert!(formatted.contains("\nwasm stdout:\nstdout-line\n"));
        assert!(formatted.contains("\nwasm stderr:\nstderr-line\n"));
    }

    #[test]
    fn format_deploy_structured_error_without_wasm_sections_keeps_legacy_shape() {
        let error = sample_structured_error(ErrorCode::Internal, false);
        let formatted = format_deploy_structured_error(&error);
        assert_eq!(formatted, "error (Internal) at upload");
    }

    fn sample_server_error(code: ErrorCode, retryable: bool) -> anyhow::Error {
        ServerResponseError {
            error: sample_structured_error(code, retryable),
        }
        .into()
    }

    #[test]
    fn upload_ranges_for_partial_requires_missing_ranges() {
        let err = upload_ranges_for_prepare(ArtifactStatus::Partial, &[], 1024)
            .expect_err("partial without missing_ranges must fail");
        assert!(err.to_string().contains("missing_ranges"));
    }

    #[test]
    fn build_upload_chunk_plan_uses_requested_ranges_only() {
        let ranges = vec![
            ByteRange {
                offset: 0,
                length: 4,
            },
            ByteRange {
                offset: 10,
                length: 3,
            },
        ];
        let plan = build_upload_chunk_plan(&ranges, 32, 2).expect("chunk plan should build");
        assert_eq!(plan, vec![(0, 2), (2, 2), (10, 2), (12, 1)]);
    }

    #[test]
    fn build_upload_chunk_plan_rejects_out_of_bounds_range() {
        let ranges = vec![ByteRange {
            offset: 8,
            length: 4,
        }];
        let err = build_upload_chunk_plan(&ranges, 10, 2)
            .expect_err("range outside artifact size must fail");
        assert!(err.to_string().contains("outside artifact size"));
    }

    #[test]
    fn idempotency_key_is_stable_for_same_payload() {
        let target = BTreeMap::from([(
            "remote".to_string(),
            "ssh://imagod.local?socket=/run/imago/imagod.sock".to_string(),
        )]);
        let policy = BTreeMap::from([("rollout".to_string(), "safe".to_string())]);

        let first = build_idempotency_key(
            "svc",
            "cli",
            &target,
            &policy,
            "digest-a",
            1024,
            "manifest-a",
        );
        let second = build_idempotency_key(
            "svc",
            "cli",
            &target,
            &policy,
            "digest-a",
            1024,
            "manifest-a",
        );

        assert_eq!(first, second);
        assert!(first.starts_with("deploy:"));
        assert_eq!(first.len(), "deploy:".len() + 64);
    }

    #[test]
    fn idempotency_key_changes_when_target_changes() {
        let key_a = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::from([(
                "remote".to_string(),
                "ssh://imagod-a?socket=/run/imago/imagod.sock".to_string(),
            )]),
            &BTreeMap::new(),
            "digest-a",
            1024,
            "manifest-a",
        );
        let key_b = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::from([(
                "remote".to_string(),
                "ssh://imagod-b?socket=/run/imago/imagod.sock".to_string(),
            )]),
            &BTreeMap::new(),
            "digest-a",
            1024,
            "manifest-a",
        );

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn idempotency_key_changes_when_policy_changes() {
        let key_a = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::new(),
            &BTreeMap::from([("rollout".to_string(), "safe".to_string())]),
            "digest-a",
            1024,
            "manifest-a",
        );
        let key_b = build_idempotency_key(
            "svc",
            "cli",
            &BTreeMap::new(),
            &BTreeMap::from([("rollout".to_string(), "fast".to_string())]),
            "digest-a",
            1024,
            "manifest-a",
        );

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn retry_classification_retries_busy_or_internal() {
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Busy,
            false
        )));
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Busy,
            true
        )));
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Internal,
            true
        )));
    }

    #[test]
    fn retry_classification_does_not_retry_bad_request_or_unauthorized() {
        assert!(!should_retry_upload_error(&sample_server_error(
            ErrorCode::BadRequest,
            true
        )));
        assert!(!should_retry_upload_error(&sample_server_error(
            ErrorCode::Unauthorized,
            true
        )));
    }

    #[test]
    fn retry_backoff_is_bounded_and_increasing() {
        assert_eq!(retry_backoff_duration(1), Duration::from_millis(250));
        assert_eq!(retry_backoff_duration(2), Duration::from_millis(500));
        assert_eq!(retry_backoff_duration(3), Duration::from_millis(1000));
        assert_eq!(retry_backoff_duration(4), Duration::from_millis(1000));
    }

    #[test]
    fn deploy_stream_retry_backoff_is_bounded() {
        assert_eq!(
            deploy_stream_retry_backoff(1),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            deploy_stream_retry_backoff(2),
            Some(Duration::from_millis(250))
        );
        assert_eq!(deploy_stream_retry_backoff(3), None);
    }

    #[test]
    fn command_start_retry_policy_never_retries_request_stream() {
        assert_eq!(
            request_stream_retry_backoff(RequestStreamRetryPolicy::CommandStartNoRetry, 1),
            None
        );
        assert_eq!(
            request_stream_max_attempts(RequestStreamRetryPolicy::CommandStartNoRetry),
            1
        );
    }

    #[test]
    fn request_stream_retry_policy_uses_standard_backoff_for_non_command_start() {
        assert_eq!(
            request_stream_retry_backoff(RequestStreamRetryPolicy::Standard, 1),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            request_stream_retry_backoff(RequestStreamRetryPolicy::Standard, 2),
            Some(Duration::from_millis(250))
        );
    }

    #[test]
    fn command_start_retry_policy_disables_request_stream_read_timeout() {
        assert_eq!(
            request_stream_read_timeout(
                RequestStreamRetryPolicy::CommandStartNoRetry,
                Duration::from_secs(15)
            ),
            None
        );
    }

    #[test]
    fn standard_retry_policy_keeps_request_stream_read_timeout() {
        assert_eq!(
            request_stream_read_timeout(
                RequestStreamRetryPolicy::Standard,
                Duration::from_secs(15)
            ),
            Some(Duration::from_secs(15))
        );
    }

    #[test]
    fn retry_classification_does_not_retry_when_server_marks_non_retryable() {
        assert!(!should_retry_upload_error(&sample_server_error(
            ErrorCode::Internal,
            false
        )));
    }

    #[test]
    fn retry_classification_retries_busy_even_when_server_marks_non_retryable() {
        assert!(should_retry_upload_error(&sample_server_error(
            ErrorCode::Busy,
            false
        )));
    }

    #[test]
    fn retry_classification_does_not_retry_unstructured_unauthorized_error() {
        let err = anyhow!(
            "server error: public key authentication failed (E_UNAUTHORIZED) at transport.connect"
        );
        assert!(!should_retry_upload_error(&err));
    }

    #[test]
    fn retry_classification_does_not_retry_commit_not_verified_error() {
        let err: anyhow::Error = CommitNotVerifiedError.into();
        assert!(!should_retry_upload_error(&err));
    }

    #[test]
    fn truncate_log_message_never_exceeds_max_chars() {
        assert_eq!(truncate_log_message("abc", 3), "abc");
        assert_eq!(truncate_log_message("abcdef", 6), "abcdef");
        assert_eq!(truncate_log_message("abcdef", 5), "ab...");
        assert_eq!(truncate_log_message("abcdef", 3), "...");
        assert_eq!(truncate_log_message("abcdef", 2), "..");
        assert_eq!(truncate_log_message("abcdef", 0), "");
    }

    #[test]
    fn format_retry_log_message_reports_failed_attempt() {
        let message = format_retry_log_message(1, 4, Duration::from_millis(250), "E_BUSY");
        assert!(message.contains("upload attempt 1/4 failed"));
        assert!(message.contains("retrying in 250ms"));
        assert!(message.contains("reason=E_BUSY"));
    }

    #[test]
    fn request_stream_retry_log_message_reports_reason() {
        let message = format_request_stream_retry_log_message(
            1,
            3,
            Duration::from_millis(100),
            "request stream read timed out",
        );
        assert!(message.contains("request stream attempt 1/3 failed"));
        assert!(message.contains("retrying in 100ms"));
        assert!(message.contains("reason=request stream read timed out"));
    }

    #[test]
    fn request_stream_failure_summary_includes_first_and_last_reason_when_different() {
        let message = format_request_stream_failure_summary(
            3,
            Some("request stream read timed out"),
            "connection reset by peer",
        );
        assert!(message.contains("after 3 attempts"));
        assert!(message.contains("first_failure=request stream read timed out"));
        assert!(message.contains("last_failure=connection reset by peer"));
    }

    #[test]
    fn deploy_phase_detail_formats_phase_fraction() {
        assert_eq!(
            deploy_phase_detail(DEPLOY_PHASE_BUILD, "building project and manifest"),
            "phase 1/8 building project and manifest"
        );
        assert_eq!(
            deploy_phase_detail(DEPLOY_PHASE_UPLOAD, "uploading artifact"),
            "phase 6/8 uploading artifact"
        );
    }

    #[test]
    fn deploy_command_phase_is_fixed_to_eight_of_eight() {
        assert_eq!(
            deploy_phase_detail(DEPLOY_PHASE_COMMAND, "sending deploy command"),
            "phase 8/8 sending deploy command"
        );
        assert_eq!(
            deploy_phase_detail(DEPLOY_PHASE_COMMAND, "remote progress"),
            "phase 8/8 remote progress"
        );
    }

    #[test]
    fn clear_deploy_spinner_before_follow_when_not_detached() {
        assert!(should_clear_deploy_spinner_before_follow(false));
    }

    #[test]
    fn keep_deploy_spinner_when_detached() {
        assert!(!should_clear_deploy_spinner_before_follow(true));
    }

    #[test]
    fn deploy_build_preview_formats_dimmed_single_line() {
        let line = build::BuildCommandLogLine {
            stream: build::BuildCommandLogStream::Stdout,
            line: "building crate-a".to_string(),
        };
        let preview = format_deploy_build_preview(&line);
        assert!(preview.contains("phase 1/8 building project and manifest"));
        assert!(preview.contains("\u{1b}[2m  > [stdout] building crate-a\u{1b}[0m"));
    }

    #[test]
    fn build_failure_log_omits_stream_label_for_copy_paste() {
        let line = build::BuildCommandLogLine {
            stream: build::BuildCommandLogStream::Stderr,
            line: "error: expected `;`".to_string(),
        };
        assert_eq!(format_build_failure_log(&line), "  > error: expected `;`");
    }

    #[test]
    fn build_failure_footer_line_mentions_abort() {
        assert_eq!(
            build_failure_footer_line(),
            "build.command failed with errors; deploy aborted"
        );
    }

    #[test]
    fn extract_build_failure_logs_finds_nested_build_error() {
        let lines = vec![build::BuildCommandLogLine {
            stream: build::BuildCommandLogStream::Stderr,
            line: "compile error: expected `;`".to_string(),
        }];
        let err = anyhow::Error::new(build::BuildCommandFailure::new(Some(7), lines.clone()))
            .context("build stage failed");

        let extracted = extract_build_failure_logs(&err).expect("build logs should be found");
        assert_eq!(extracted, lines.as_slice());
    }

    #[test]
    fn build_connected_target_metadata_carries_expected_fields() {
        let target = build::DeployTargetConfig {
            remote: "ssh://root@imagod.local:2222?socket=/tmp/imagod.sock".to_string(),
            ssh_remote: build::SshTargetRemote {
                user: Some("root".to_string()),
                host: "imagod.local".to_string(),
                port: Some(2222),
                socket_path: Some("/tmp/imagod.sock".to_string()),
            },
        };
        let metadata = build_connected_target_metadata(
            &target,
            "imagod.local",
            "imagod.local:4443",
            "127.0.0.1:4443",
        );

        assert_eq!(metadata.authority, "imagod.local:4443");
        assert_eq!(metadata.resolved_addr.to_string(), "127.0.0.1:4443");
        assert_eq!(metadata.configured_host, "imagod.local");
        assert_eq!(
            metadata.remote_input,
            "ssh://root@imagod.local:2222?socket=/tmp/imagod.sock"
        );
    }

    #[test]
    fn hello_summary_from_response_reflects_server_limits() {
        let response = HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod/0.1.0".to_string(),
            server_protocol_version: "0.1.0".to_string(),
            supported_protocol_version_range: ">=0.1.0,<0.2.0".to_string(),
            compatibility_announcement: None,
            features: vec!["logs.request".to_string()],
            limits: BTreeMap::from([
                ("chunk_size".to_string(), "4096".to_string()),
                ("max_inflight_chunks".to_string(), "8".to_string()),
                ("deploy_stream_timeout_secs".to_string(), "20".to_string()),
            ]),
        };
        let summary = hello_summary_from_response(&response);

        assert_eq!(summary.server_version, "imagod/0.1.0");
        assert_eq!(
            summary.limits.get("chunk_size").map(String::as_str),
            Some("4096")
        );
        assert_eq!(
            summary
                .limits
                .get("max_inflight_chunks")
                .map(String::as_str),
            Some("8")
        );
        assert_eq!(
            summary
                .limits
                .get("deploy_stream_timeout_secs")
                .map(String::as_str),
            Some("20")
        );
    }

    #[test]
    fn normalize_bundle_entry_path_rejects_unsafe_values() {
        assert!(normalize_bundle_entry_path("../evil.wasm", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("/etc/passwd", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("C:\\evil.wasm", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("..\\evil.wasm", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("", "manifest.main").is_err());
        assert!(normalize_bundle_entry_path("app/main.wasm", "manifest.main").is_ok());
    }

    #[test]
    fn build_artifact_bundle_file_includes_hashed_main_wasm() {
        let root = std::env::temp_dir().join(format!("imago-cli-bundle-hashed-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("build")).expect("build dir should be created");
        fs::write(root.join("build/manifest.json"), "{}").expect("manifest source should exist");

        let hashed_main =
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef-svc.wasm";
        fs::write(root.join("build").join(hashed_main), b"wasm").expect("hashed main should exist");

        let manifest = Manifest {
            name: "svc".to_string(),
            main: hashed_main.to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![],
        };

        let bundle = build_artifact_bundle_file(
            &manifest,
            Path::new("build/manifest.json"),
            &root,
            &BTreeMap::new(),
        )
        .expect("bundle should be created");

        let file = std::fs::File::open(bundle.path()).expect("bundle file should open");
        let mut archive = tar::Archive::new(file);
        let mut names = Vec::new();
        for entry in archive.entries().expect("tar entries should be readable") {
            let entry = entry.expect("tar entry should read");
            let path = entry.path().expect("entry path should parse");
            names.push(path.to_string_lossy().to_string());
        }

        assert!(names.contains(&"manifest.json".to_string()));
        assert!(names.contains(&hashed_main.to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_artifact_bundle_file_rejects_unsafe_manifest_main() {
        let root = std::env::temp_dir().join(format!("imago-cli-bundle-main-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("build")).expect("build dir should be created");
        fs::write(root.join("build/manifest.json"), "{}").expect("manifest source should exist");

        let manifest = Manifest {
            name: "svc".to_string(),
            main: "../evil.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![],
        };

        let err = build_artifact_bundle_file(
            &manifest,
            Path::new("build/manifest.json"),
            &root,
            &BTreeMap::new(),
        )
        .expect_err("unsafe manifest.main should be rejected");
        assert!(err.to_string().contains("manifest.main"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_artifact_bundle_file_rejects_unsafe_asset_path() {
        let root = std::env::temp_dir().join(format!("imago-cli-bundle-asset-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("build")).expect("build dir should be created");
        fs::write(root.join("build/manifest.json"), "{}").expect("manifest source should exist");
        fs::write(root.join("build/main.wasm"), b"00").expect("main wasm should exist");

        let manifest = Manifest {
            name: "svc".to_string(),
            main: "main.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![ManifestAsset {
                path: "../secret.txt".to_string(),
            }],
            dependencies: vec![],
        };

        let err = build_artifact_bundle_file(
            &manifest,
            Path::new("build/manifest.json"),
            &root,
            &BTreeMap::new(),
        )
        .expect_err("unsafe asset path should be rejected");
        assert!(err.to_string().contains("assets[].path"));

        let _ = fs::remove_dir_all(root);
    }
}
