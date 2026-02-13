//! Deploy protocol session handler and message dispatch implementation.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use imago_protocol::{
    ArtifactPushRequest, CommandCancelRequest, CommandEvent, CommandEventType, CommandPayload,
    CommandStartRequest, CommandStartResponse, CommandState, CommandType, DeployPrepareRequest,
    LogChunk, LogEnd, LogError, LogErrorCode, LogRequest, LogStreamKind, MessageType,
    ProtocolEnvelope, StateRequest, StructuredError, Validate, from_cbor, to_cbor,
};
use imagod_common::ImagodError;
use imagod_config::ImagodConfig;
use imagod_control::{
    ArtifactStore, OperationManager, Orchestrator, ServiceLogEvent, ServiceLogStream,
    ServiceLogSubscription, SpawnTransition,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;
use web_transport_quinn::{SendStream, Session};

const MAX_STREAM_BYTES: usize = 1024 * 1024 * 16;
const STREAM_READ_TIMEOUT_SECS: u64 = 30;
const LOG_DATAGRAM_TARGET_BYTES: usize = 1024;

/// JSON-backed envelope type used by stream decode/encode flow.
type Envelope = ProtocolEnvelope<Value>;

#[derive(Clone)]
/// Handles one WebTransport session and dispatches protocol messages.
pub struct ProtocolHandler {
    config: Arc<ImagodConfig>,
    artifacts: ArtifactStore,
    operations: OperationManager,
    orchestrator: Orchestrator,
    shutdown_requested: Arc<AtomicBool>,
}

impl ProtocolHandler {
    /// Creates a protocol handler with shared manager dependencies.
    pub fn new(
        config: Arc<ImagodConfig>,
        artifacts: ArtifactStore,
        operations: OperationManager,
        orchestrator: Orchestrator,
    ) -> Self {
        Self {
            config,
            artifacts,
            operations,
            orchestrator,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Rejects new start commands and transitions handler into shutdown mode.
    pub fn begin_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
    }

    /// Serves one WebTransport session until peer closes it.
    pub async fn handle_session(&self, session: Session) -> Result<(), ImagodError> {
        loop {
            let (mut send, mut recv) = match session.accept_bi().await {
                Ok(streams) => streams,
                Err(_) => break,
            };

            let buf = match read_stream_with_timeout(
                recv.read_to_end(MAX_STREAM_BYTES),
                Duration::from_secs(STREAM_READ_TIMEOUT_SECS),
            )
            .await
            {
                Ok(buf) => buf,
                Err(err) => {
                    let envelope = error_envelope(
                        MessageType::CommandEvent,
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        err.to_structured(),
                    );
                    write_envelope(&mut send, &envelope).await?;
                    finish_stream(&mut send)?;
                    continue;
                }
            };

            let envelopes = match parse_stream_envelopes(&buf) {
                Ok(v) => v,
                Err(err) => {
                    let envelope = error_envelope(
                        MessageType::CommandEvent,
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        err.to_structured(),
                    );
                    write_envelope(&mut send, &envelope).await?;
                    finish_stream(&mut send)?;
                    continue;
                }
            };

            if envelopes.is_empty() {
                finish_stream(&mut send)?;
                continue;
            }

            if let Err(err) = ensure_single_request_envelope(&envelopes) {
                let first = &envelopes[0];
                let response = error_envelope(
                    response_message_type_for_request(first.message_type),
                    first.request_id,
                    first.correlation_id,
                    err.to_structured(),
                );
                write_envelope(&mut send, &response).await?;
                finish_stream(&mut send)?;
                continue;
            }

            let request = envelopes[0].clone();
            if request.message_type == MessageType::CommandStart {
                self.handle_command_start(request, &mut send).await?;
                finish_stream(&mut send)?;
                continue;
            }
            if request.message_type == MessageType::LogsRequest {
                if let Err(err) = self
                    .handle_logs_request(session.clone(), request.clone(), &mut send)
                    .await
                {
                    let response = error_envelope(
                        MessageType::LogsRequest,
                        request.request_id,
                        request.correlation_id,
                        err.to_structured(),
                    );
                    write_envelope(&mut send, &response).await?;
                }
                finish_stream(&mut send)?;
                continue;
            }

            let response = match self.handle_single(request.clone()).await {
                Ok(resp) => resp,
                Err(err) => error_envelope(
                    response_message_type_for_request(request.message_type),
                    request.request_id,
                    request.correlation_id,
                    err.to_structured(),
                ),
            };
            write_envelope(&mut send, &response).await?;
            finish_stream(&mut send)?;
        }

        Ok(())
    }

    /// Reaps finished services via orchestrator.
    pub async fn reap_finished_services(&self) {
        self.orchestrator.reap_finished_services().await;
    }

    /// Returns whether any managed services are currently alive.
    pub async fn has_live_services(&self) -> bool {
        self.orchestrator.has_live_services().await
    }

    /// Stops all managed services.
    pub async fn stop_all_services(&self, force: bool) -> Vec<(String, ImagodError)> {
        self.orchestrator.stop_all_services(force).await
    }

    /// Dispatches non-command-start requests to the corresponding handler.
    async fn handle_single(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        match request.message_type {
            MessageType::HelloNegotiate => self.handle_hello(request),
            MessageType::DeployPrepare => self.handle_prepare(request).await,
            MessageType::ArtifactPush => self.handle_push(request).await,
            MessageType::ArtifactCommit => self.handle_commit(request).await,
            MessageType::StateRequest => self.handle_state_request(request).await,
            MessageType::CommandCancel => self.handle_command_cancel(request).await,
            _ => Err(ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "dispatch",
                "unsupported message type",
            )),
        }
    }

    fn handle_hello(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: imago_protocol::HelloNegotiateRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("hello.negotiate", e.to_string()))?;

        let accepted =
            is_compatible_date_match(&payload.compatibility_date, &self.config.compatibility_date);
        let mut limits = BTreeMap::new();
        limits.insert(
            "chunk_size".to_string(),
            self.config.runtime.chunk_size.to_string(),
        );
        limits.insert(
            "max_inflight_chunks".to_string(),
            self.config.runtime.max_inflight_chunks.to_string(),
        );
        limits.insert(
            "max_artifact_size_bytes".to_string(),
            self.config.runtime.max_artifact_size_bytes.to_string(),
        );
        limits.insert(
            "upload_session_ttl".to_string(),
            format!("{}s", self.config.runtime.upload_session_ttl_secs),
        );

        response_envelope(
            MessageType::HelloNegotiate,
            request.request_id,
            request.correlation_id,
            &imago_protocol::HelloNegotiateResponse {
                accepted,
                server_version: self.config.server_version.clone(),
                features: vec![
                    "hello.negotiate".to_string(),
                    "deploy.prepare".to_string(),
                    "artifact.push".to_string(),
                    "artifact.commit".to_string(),
                    "command.start".to_string(),
                    "command.event".to_string(),
                    "state.request".to_string(),
                    "command.cancel".to_string(),
                    "logs.request".to_string(),
                    "logs.chunk".to_string(),
                    "logs.end".to_string(),
                ],
                limits,
            },
        )
    }

    async fn handle_prepare(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: DeployPrepareRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("deploy.prepare", e.to_string()))?;

        let response = self.artifacts.prepare(payload).await?;
        response_envelope(
            MessageType::DeployPrepare,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_push(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: ArtifactPushRequest = payload_as(&request)?;
        validate_push_payload(&payload)?;

        let response = self.artifacts.push(payload).await?;
        response_envelope(
            MessageType::ArtifactPush,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_commit(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: imago_protocol::ArtifactCommitRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("artifact.commit", e.to_string()))?;

        let response = self.artifacts.commit(payload).await?;
        response_envelope(
            MessageType::ArtifactCommit,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_state_request(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: StateRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("state.request", e.to_string()))?;

        let response = self
            .operations
            .snapshot_running(&payload.request_id)
            .await?;
        response_envelope(
            MessageType::StateResponse,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    async fn handle_command_cancel(&self, request: Envelope) -> Result<Envelope, ImagodError> {
        let payload: CommandCancelRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("command.cancel", e.to_string()))?;

        let response = self.operations.request_cancel(&payload.request_id).await?;
        response_envelope(
            MessageType::CommandCancel,
            request.request_id,
            request.correlation_id,
            &response,
        )
    }

    /// Handles `logs.request`, returns stream ACK and starts datagram forwarding.
    async fn handle_logs_request(
        &self,
        session: Session,
        request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError> {
        let payload: LogRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("logs.request", e.to_string()))?;

        let service_names = match payload.process_id {
            Some(process_id) => vec![process_id],
            None => {
                let names = self.orchestrator.running_service_names().await;
                if names.is_empty() {
                    return Err(ImagodError::new(
                        imago_protocol::ErrorCode::NotFound,
                        "logs.request",
                        "no running services are available",
                    ));
                }
                names
            }
        };

        let mut subscriptions = Vec::with_capacity(service_names.len());
        for service_name in &service_names {
            let subscription = self
                .orchestrator
                .open_logs(service_name, payload.tail_lines, payload.follow)
                .await?;
            subscriptions.push(subscription);
        }

        #[derive(Serialize)]
        struct LogsRequestAck {
            accepted: bool,
            process_ids: Vec<String>,
            follow: bool,
        }

        let ack = response_envelope(
            MessageType::LogsRequest,
            request.request_id,
            request.correlation_id,
            &LogsRequestAck {
                accepted: true,
                process_ids: service_names,
                follow: payload.follow,
            },
        )?;
        write_envelope(send, &ack).await?;

        tokio::spawn(async move {
            run_logs_forwarder(
                session,
                request.request_id,
                request.correlation_id,
                subscriptions,
            )
            .await;
        });

        Ok(())
    }

    /// Handles `command.start` and emits accepted/progress/terminal events.
    async fn handle_command_start(
        &self,
        request: Envelope,
        send: &mut SendStream,
    ) -> Result<(), ImagodError> {
        let payload: CommandStartRequest = payload_as(&request)?;
        payload
            .validate()
            .map_err(|e| bad_request("command.start", e.to_string()))?;

        ensure_command_start_request_id_match(request.request_id, payload.request_id)?;
        ensure_command_start_allowed(&self.shutdown_requested)?;
        let operation_id = request.request_id;

        self.operations
            .start(operation_id, payload.command_type)
            .await?;

        let accepted = response_envelope(
            MessageType::CommandStart,
            request.request_id,
            request.correlation_id,
            &CommandStartResponse { accepted: true },
        )?;
        write_envelope(send, &accepted).await?;

        let accepted_event = event_envelope(
            operation_id,
            request.correlation_id,
            CommandEventType::Accepted,
            payload.command_type,
            None,
            None,
        )?;
        write_envelope(send, &accepted_event).await?;

        self.operations
            .set_state(&operation_id, CommandState::Running, "starting")
            .await?;
        let running_event = event_envelope(
            operation_id,
            request.correlation_id,
            CommandEventType::Progress,
            payload.command_type,
            Some("starting".to_string()),
            None,
        )?;
        write_envelope(send, &running_event).await?;

        let spawn_transition = self
            .operations
            .mark_spawned_if_not_canceled(&operation_id, "spawned")
            .await?;
        if spawn_transition == SpawnTransition::Canceled {
            let canceled = event_envelope(
                operation_id,
                request.correlation_id,
                CommandEventType::Canceled,
                payload.command_type,
                Some("canceled".to_string()),
                None,
            )?;
            let canceled_write = write_envelope(send, &canceled).await;
            finalize_operation_after_terminal_event(
                &self.operations,
                &operation_id,
                CommandState::Canceled,
                "canceled",
                canceled_write,
            )
            .await?;
            return Ok(());
        }

        let command_result = match (&payload.command_type, &payload.payload) {
            (CommandType::Deploy, CommandPayload::Deploy(deploy_payload)) => self
                .orchestrator
                .deploy(deploy_payload)
                .await
                .map(|summary| {
                    (
                        format!("release:{}:{}", summary.service_name, summary.release_hash),
                        "spawned".to_string(),
                    )
                }),
            (CommandType::Run, CommandPayload::Run(run_payload)) => {
                self.orchestrator.run(run_payload).await.map(|summary| {
                    (
                        format!("running:{}:{}", summary.service_name, summary.release_hash),
                        "spawned".to_string(),
                    )
                })
            }
            (CommandType::Stop, CommandPayload::Stop(stop_payload)) => {
                self.orchestrator.stop(stop_payload).await.map(|summary| {
                    (
                        format!("stopped:{}", summary.service_name),
                        "completed".to_string(),
                    )
                })
            }
            _ => Err(ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "command.start",
                "payload does not match command_type",
            )),
        };

        match command_result {
            Ok((progress_stage, success_stage)) => {
                let success_stage_for_event = success_stage.clone();
                let progress = event_envelope(
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Progress,
                    payload.command_type,
                    Some(progress_stage),
                    None,
                )?;
                write_envelope(send, &progress).await?;

                let succeeded = event_envelope(
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Succeeded,
                    payload.command_type,
                    Some(success_stage_for_event),
                    None,
                )?;
                let succeeded_write = write_envelope(send, &succeeded).await;
                finalize_operation_after_terminal_event(
                    &self.operations,
                    &operation_id,
                    CommandState::Succeeded,
                    success_stage,
                    succeeded_write,
                )
                .await?;
            }
            Err(err) => {
                let failed = event_envelope(
                    operation_id,
                    request.correlation_id,
                    CommandEventType::Failed,
                    payload.command_type,
                    Some("failed".to_string()),
                    Some(err.to_structured()),
                )?;
                let failed_write = write_envelope(send, &failed).await;
                finalize_operation_after_terminal_event(
                    &self.operations,
                    &operation_id,
                    CommandState::Failed,
                    "failed",
                    failed_write,
                )
                .await?;
            }
        }

        Ok(())
    }
}

async fn run_logs_forwarder(
    session: Session,
    request_id: Uuid,
    correlation_id: Uuid,
    subscriptions: Vec<ServiceLogSubscription>,
) {
    if subscriptions.is_empty() {
        return;
    }

    let max_datagram_size = session.max_datagram_size();
    let fallback_process_id = subscriptions[0].service_name.clone();
    let service_names = subscriptions
        .iter()
        .map(|subscription| subscription.service_name.clone())
        .collect::<Vec<_>>();
    let mut seq = 0u64;
    let mut last_process_id: Option<String> = None;
    let chunk_size = match fixed_log_chunk_size(
        request_id,
        correlation_id,
        max_datagram_size,
        &service_names,
    ) {
        Ok(size) => size,
        Err(err) => {
            let _ = send_logs_end_datagram(
                &session,
                request_id,
                correlation_id,
                max_datagram_size,
                seq,
                Some(log_error_from_imagod_error(&err)),
            );
            return;
        }
    };
    let sender = LogsDatagramSender::new(
        &session,
        request_id,
        correlation_id,
        max_datagram_size,
        chunk_size,
    );

    let stream_result = stream_logs_datagrams(
        &session,
        &sender,
        subscriptions,
        &mut seq,
        &mut last_process_id,
    )
    .await;

    match stream_result {
        Ok(()) => {
            let terminal_process = last_process_id.unwrap_or(fallback_process_id);
            let _ = sender.send_single_log_chunk(
                &mut seq,
                &terminal_process,
                LogStreamKind::Composite,
                Vec::new(),
                true,
            );
            let _ = sender.send_logs_end_datagram(seq, None);
        }
        Err(err) => {
            let _ = sender.send_logs_end_datagram(seq, Some(log_error_from_imagod_error(&err)));
        }
    }
}

struct LogsDatagramSender<'a> {
    session: &'a Session,
    request_id: Uuid,
    correlation_id: Uuid,
    max_datagram_size: usize,
    chunk_size: usize,
}

impl<'a> LogsDatagramSender<'a> {
    fn new(
        session: &'a Session,
        request_id: Uuid,
        correlation_id: Uuid,
        max_datagram_size: usize,
        chunk_size: usize,
    ) -> Self {
        Self {
            session,
            request_id,
            correlation_id,
            max_datagram_size,
            chunk_size,
        }
    }

    fn send_log_data_chunks(
        &self,
        seq: &mut u64,
        process_id: &str,
        stream_kind: LogStreamKind,
        bytes: &[u8],
        last_process_id: &mut Option<String>,
    ) -> Result<(), ImagodError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.chunk_size == 0 {
            return Err(ImagodError::new(
                imago_protocol::ErrorCode::Internal,
                "logs.datagram",
                "computed logs chunk size must be greater than zero",
            ));
        }

        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = bytes.len().min(offset.saturating_add(self.chunk_size));
            self.send_single_log_chunk(
                seq,
                process_id,
                stream_kind,
                bytes[offset..end].to_vec(),
                false,
            )?;
            *last_process_id = Some(process_id.to_string());
            offset = end;
        }

        Ok(())
    }

    fn send_single_log_chunk(
        &self,
        seq: &mut u64,
        process_id: &str,
        stream_kind: LogStreamKind,
        bytes: Vec<u8>,
        is_last: bool,
    ) -> Result<(), ImagodError> {
        let chunk = LogChunk {
            request_id: self.request_id,
            seq: *seq,
            process_id: process_id.to_string(),
            stream_kind,
            bytes,
            is_last,
        };
        let envelope = ProtocolEnvelope::new(
            MessageType::LogsChunk,
            self.request_id,
            self.correlation_id,
            chunk,
        );
        send_datagram_envelope(self.session, &envelope, self.max_datagram_size)?;
        *seq = seq.saturating_add(1);
        Ok(())
    }

    fn send_logs_end_datagram(&self, seq: u64, error: Option<LogError>) -> Result<(), ImagodError> {
        send_logs_end_datagram(
            self.session,
            self.request_id,
            self.correlation_id,
            self.max_datagram_size,
            seq,
            error,
        )
    }
}

fn send_logs_end_datagram(
    session: &Session,
    request_id: Uuid,
    correlation_id: Uuid,
    max_datagram_size: usize,
    seq: u64,
    error: Option<LogError>,
) -> Result<(), ImagodError> {
    let end = LogEnd {
        request_id,
        seq,
        error,
    };
    let envelope = ProtocolEnvelope::new(MessageType::LogsEnd, request_id, correlation_id, end);
    send_datagram_envelope(session, &envelope, max_datagram_size)
}

async fn stream_logs_datagrams(
    session: &Session,
    sender: &LogsDatagramSender<'_>,
    subscriptions: Vec<ServiceLogSubscription>,
    seq: &mut u64,
    last_process_id: &mut Option<String>,
) -> Result<(), ImagodError> {
    for subscription in &subscriptions {
        sender.send_log_data_chunks(
            seq,
            &subscription.service_name,
            LogStreamKind::Composite,
            &subscription.snapshot_bytes,
            last_process_id,
        )?;
    }

    let mut follow_targets = subscriptions
        .into_iter()
        .filter_map(|subscription| {
            subscription
                .receiver
                .map(|receiver| (subscription.service_name, receiver))
        })
        .collect::<Vec<_>>();
    if follow_targets.is_empty() {
        return Ok(());
    }

    let (tx, mut rx) = mpsc::channel::<(String, ServiceLogEvent)>(128);
    let mut forward_tasks = Vec::new();
    for (service_name, mut receiver) in follow_targets.drain(..) {
        let tx = tx.clone();
        let handle = tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if tx.send((service_name.clone(), event)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        forward_tasks.push(handle);
    }
    drop(tx);

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                let Some((service_name, event)) = maybe_event else {
                    break;
                };
                sender.send_log_data_chunks(
                    seq,
                    &service_name,
                    service_log_stream_to_protocol(event.stream),
                    &event.bytes,
                    last_process_id,
                )?;
            }
            _ = session.closed() => break,
        }
    }

    for task in forward_tasks {
        task.abort();
    }

    Ok(())
}

fn send_datagram_envelope<T: Serialize>(
    session: &Session,
    envelope: &ProtocolEnvelope<T>,
    max_datagram_size: usize,
) -> Result<(), ImagodError> {
    let bytes = to_cbor(envelope).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!("failed to encode datagram payload: {e}"),
        )
    })?;
    if bytes.len() > max_datagram_size {
        return Err(ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!(
                "datagram payload too large: size={} max={}",
                bytes.len(),
                max_datagram_size
            ),
        ));
    }
    session.send_datagram(bytes.into()).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!("failed to send datagram: {e}"),
        )
    })
}

fn fixed_log_chunk_size(
    request_id: Uuid,
    correlation_id: Uuid,
    max_datagram_size: usize,
    service_names: &[String],
) -> Result<usize, ImagodError> {
    let process_id = service_names
        .iter()
        .max_by_key(|name| name.len())
        .cloned()
        .unwrap_or_else(|| "logs".to_string());
    let probe = LogChunk {
        request_id,
        seq: u64::MAX,
        process_id,
        stream_kind: LogStreamKind::Composite,
        bytes: Vec::new(),
        is_last: false,
    };
    let envelope = ProtocolEnvelope::new(MessageType::LogsChunk, request_id, correlation_id, probe);
    let overhead = to_cbor(&envelope).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!("failed to encode datagram probe: {e}"),
        )
    })?;
    let computed_limit = max_datagram_size.saturating_sub(overhead.len().saturating_add(2));
    let chunk_size = computed_limit.min(LOG_DATAGRAM_TARGET_BYTES);
    if chunk_size == 0 {
        return Err(ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "logs.datagram",
            format!(
                "datagram size is too small for logs payload: max={} overhead={}",
                max_datagram_size,
                overhead.len()
            ),
        ));
    }

    Ok(chunk_size)
}

fn service_log_stream_to_protocol(stream: ServiceLogStream) -> LogStreamKind {
    match stream {
        ServiceLogStream::Stdout => LogStreamKind::Stdout,
        ServiceLogStream::Stderr => LogStreamKind::Stderr,
    }
}

fn log_error_from_imagod_error(err: &ImagodError) -> LogError {
    let code = match err.code {
        imago_protocol::ErrorCode::NotFound => LogErrorCode::ProcessNotFound,
        imago_protocol::ErrorCode::Unauthorized => LogErrorCode::PermissionDenied,
        _ => LogErrorCode::Internal,
    };

    LogError {
        code,
        message: err.message.clone(),
    }
}

fn response_envelope<T: Serialize>(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    payload: &T,
) -> Result<Envelope, ImagodError> {
    let payload = serde_json::to_value(payload)
        .map_err(|e| bad_request("protocol", format!("payload encode failed: {e}")))?;
    Ok(Envelope {
        message_type,
        request_id,
        correlation_id,
        payload,
        error: None,
    })
}

fn error_envelope(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    error: StructuredError,
) -> Envelope {
    Envelope {
        message_type,
        request_id,
        correlation_id,
        payload: Value::Null,
        error: Some(error),
    }
}

fn response_message_type_for_request(request_type: MessageType) -> MessageType {
    match request_type {
        MessageType::StateRequest => MessageType::StateResponse,
        _ => request_type,
    }
}

fn payload_as<T: DeserializeOwned>(request: &Envelope) -> Result<T, ImagodError> {
    serde_json::from_value(request.payload.clone())
        .map_err(|e| bad_request("protocol", format!("request payload decode failed: {e}")))
}

/// Decodes one stream payload into protocol envelopes.
fn parse_stream_envelopes(buf: &[u8]) -> Result<Vec<Envelope>, ImagodError> {
    let frames = decode_frames(buf)?;
    frames
        .iter()
        .map(|frame| {
            let envelope = from_cbor::<Envelope>(frame)
                .map_err(|e| bad_request("protocol", format!("invalid frame payload: {e}")))?;
            ensure_non_nil_envelope_ids(&envelope)?;
            Ok(envelope)
        })
        .collect()
}

async fn read_stream_with_timeout<F, E>(
    read_future: F,
    timeout_duration: Duration,
) -> Result<Vec<u8>, ImagodError>
where
    F: std::future::Future<Output = Result<Vec<u8>, E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(timeout_duration, read_future).await {
        Ok(result) => result.map_err(|e| {
            ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "session.read",
                format!("failed to read stream: {e}"),
            )
        }),
        Err(_) => Err(stream_read_timeout_error()),
    }
}

fn stream_read_timeout_error() -> ImagodError {
    ImagodError::new(
        imago_protocol::ErrorCode::OperationTimeout,
        "session.read",
        format!(
            "stream read timed out after {} seconds",
            STREAM_READ_TIMEOUT_SECS
        ),
    )
}

fn ensure_non_nil_envelope_ids(envelope: &Envelope) -> Result<(), ImagodError> {
    if envelope.request_id.is_nil() {
        return Err(bad_request("protocol", "request_id must not be nil UUID"));
    }
    if envelope.correlation_id.is_nil() {
        return Err(bad_request(
            "protocol",
            "correlation_id must not be nil UUID",
        ));
    }
    Ok(())
}

/// Ensures the stream carries at most one request envelope.
fn ensure_single_request_envelope(envelopes: &[Envelope]) -> Result<(), ImagodError> {
    if envelopes.len() > 1 {
        return Err(bad_request(
            "session.protocol",
            "multiple request envelopes on a single stream are not allowed",
        ));
    }
    Ok(())
}

async fn write_envelope(send: &mut SendStream, envelope: &Envelope) -> Result<(), ImagodError> {
    let data = to_cbor(envelope)
        .map_err(|e| bad_request("protocol", format!("cbor encode failed: {e}")))?;
    let framed = encode_frame(&data);
    send.write_all(&framed).await.map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "session.write",
            format!("failed to send frame: {e}"),
        )
    })?;
    Ok(())
}

fn finish_stream(send: &mut SendStream) -> Result<(), ImagodError> {
    send.finish().map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "session.write",
            format!("failed to finish stream: {e}"),
        )
    })
}

fn event_envelope(
    request_id: Uuid,
    correlation_id: Uuid,
    event_type: CommandEventType,
    command_type: CommandType,
    stage: Option<String>,
    error: Option<StructuredError>,
) -> Result<Envelope, ImagodError> {
    let payload = CommandEvent {
        event_type,
        request_id,
        command_type,
        timestamp: now_unix_secs(),
        stage,
        error,
    };
    response_envelope(
        MessageType::CommandEvent,
        request_id,
        correlation_id,
        &payload,
    )
}

fn now_unix_secs() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Decodes length-prefixed frame data into individual payload buffers.
fn decode_frames(value: &[u8]) -> Result<Vec<Vec<u8>>, ImagodError> {
    let mut out = Vec::new();
    let mut offset = 0usize;

    while offset < value.len() {
        if value.len() - offset < 4 {
            return Err(bad_request("protocol", "truncated frame header"));
        }

        let len = u32::from_be_bytes(
            value[offset..offset + 4]
                .try_into()
                .map_err(|_| bad_request("protocol", "invalid frame header"))?,
        ) as usize;
        offset += 4;

        if value.len() - offset < len {
            return Err(bad_request("protocol", "truncated frame payload"));
        }

        out.push(value[offset..offset + len].to_vec());
        offset += len;
    }

    Ok(out)
}

fn bad_request(stage: &str, message: impl Into<String>) -> ImagodError {
    ImagodError::new(imago_protocol::ErrorCode::BadRequest, stage, message)
}

fn is_compatible_date_match(request: &str, configured: &str) -> bool {
    request == configured
}

fn ensure_command_start_request_id_match(
    envelope_request_id: Uuid,
    payload_request_id: Uuid,
) -> Result<(), ImagodError> {
    if envelope_request_id == payload_request_id {
        return Ok(());
    }

    Err(bad_request(
        "command.start",
        "envelope request_id and payload request_id must match",
    ))
}

fn ensure_command_start_allowed(shutdown_requested: &AtomicBool) -> Result<(), ImagodError> {
    if !shutdown_requested.load(Ordering::SeqCst) {
        return Ok(());
    }

    Err(ImagodError::new(
        imago_protocol::ErrorCode::Busy,
        "command.start",
        "server is shutting down",
    ))
}

fn validate_push_payload(payload: &ArtifactPushRequest) -> Result<(), ImagodError> {
    payload
        .validate()
        .map_err(|e| bad_request("artifact.push", e.to_string()))
}

/// Finalizes operation bookkeeping after writing terminal event.
async fn finalize_operation_after_terminal_event(
    operations: &OperationManager,
    request_id: &Uuid,
    terminal_state: CommandState,
    stage: impl Into<String>,
    terminal_write_result: Result<(), ImagodError>,
) -> Result<(), ImagodError> {
    operations
        .finish(request_id, terminal_state, stage.into())
        .await;
    operations.remove(request_id).await;
    terminal_write_result
}

#[cfg(test)]
mod tests {
    use super::{
        ImagodError, OperationManager, ensure_command_start_allowed,
        ensure_command_start_request_id_match, ensure_non_nil_envelope_ids,
        ensure_single_request_envelope, finalize_operation_after_terminal_event,
        fixed_log_chunk_size, is_compatible_date_match, log_error_from_imagod_error,
        read_stream_with_timeout, response_message_type_for_request,
        service_log_stream_to_protocol, stream_read_timeout_error, validate_push_payload,
    };
    use imago_protocol::{
        ArtifactPushChunkHeader, ArtifactPushRequest, CommandState, CommandType, ErrorCode,
        LogChunk, LogErrorCode, LogStreamKind, MessageType, ProtocolEnvelope, to_cbor,
    };
    use imagod_control::ServiceLogStream;
    use serde_json::Value;
    use std::{sync::atomic::AtomicBool, time::Duration};
    use uuid::Uuid;

    #[test]
    fn accepts_same_compatibility_date() {
        assert!(is_compatible_date_match("2026-02-10", "2026-02-10"));
    }

    #[test]
    fn rejects_different_compatibility_date() {
        assert!(!is_compatible_date_match("2026-02-11", "2026-02-10"));
    }

    #[test]
    fn rejects_multiple_request_envelopes_on_single_stream() {
        let envelope = ProtocolEnvelope {
            message_type: MessageType::HelloNegotiate,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: Value::Null,
            error: None,
        };
        let result = ensure_single_request_envelope(&[envelope.clone(), envelope]);
        assert!(result.is_err());
    }

    #[test]
    fn state_request_errors_use_state_response_message_type() {
        assert_eq!(
            response_message_type_for_request(MessageType::StateRequest),
            MessageType::StateResponse
        );
    }

    #[test]
    fn non_state_request_errors_keep_original_message_type() {
        assert_eq!(
            response_message_type_for_request(MessageType::DeployPrepare),
            MessageType::DeployPrepare
        );
    }

    #[test]
    fn command_start_request_ids_must_match() {
        let envelope_request_id = Uuid::new_v4();
        let payload_request_id = Uuid::new_v4();
        let err = ensure_command_start_request_id_match(envelope_request_id, payload_request_id)
            .expect_err("mismatched request ids should be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
    }

    #[test]
    fn command_start_request_ids_can_match() {
        let request_id = Uuid::new_v4();
        let result = ensure_command_start_request_id_match(request_id, request_id);
        assert!(result.is_ok());
    }

    #[test]
    fn command_start_is_rejected_when_shutdown_requested() {
        let shutdown_requested = AtomicBool::new(true);
        let err = ensure_command_start_allowed(&shutdown_requested)
            .expect_err("shutdown mode should reject command.start");
        assert_eq!(err.code, imago_protocol::ErrorCode::Busy);
        assert_eq!(err.stage, "command.start");
        assert_eq!(err.message, "server is shutting down");
    }

    #[test]
    fn rejects_nil_request_id_in_envelope() {
        let envelope = ProtocolEnvelope {
            message_type: MessageType::HelloNegotiate,
            request_id: Uuid::nil(),
            correlation_id: Uuid::new_v4(),
            payload: Value::Null,
            error: None,
        };
        assert!(ensure_non_nil_envelope_ids(&envelope).is_err());
    }

    #[test]
    fn rejects_nil_correlation_id_in_envelope() {
        let envelope = ProtocolEnvelope {
            message_type: MessageType::HelloNegotiate,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::nil(),
            payload: Value::Null,
            error: None,
        };
        assert!(ensure_non_nil_envelope_ids(&envelope).is_err());
    }

    #[test]
    fn rejects_empty_chunk_b64_in_handle_push_validation() {
        let payload = ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: "deploy-1".to_string(),
                offset: 0,
                length: 4,
                chunk_sha256: "abcd".to_string(),
                upload_token: "token-1".to_string(),
            },
            chunk_b64: String::new(),
        };
        let err = validate_push_payload(&payload).expect_err("empty chunk_b64 should be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
    }

    #[tokio::test]
    async fn read_stream_timeout_returns_operation_timeout() {
        let err = read_stream_with_timeout(
            std::future::pending::<Result<Vec<u8>, std::io::Error>>(),
            Duration::from_millis(1),
        )
        .await
        .expect_err("pending read should timeout");
        assert_eq!(err.code, imago_protocol::ErrorCode::OperationTimeout);
    }

    #[test]
    fn stream_timeout_error_has_session_read_stage() {
        let err = stream_read_timeout_error();
        assert_eq!(err.code, imago_protocol::ErrorCode::OperationTimeout);
        assert_eq!(err.stage, "session.read");
    }

    #[tokio::test]
    async fn finalize_terminal_operation_removes_operation_even_when_stream_write_failed() {
        let operations = OperationManager::new();
        let request_id = Uuid::new_v4();
        operations
            .start(request_id, CommandType::Deploy)
            .await
            .expect("start should succeed");
        operations
            .set_state(&request_id, CommandState::Running, "running")
            .await
            .expect("state update should succeed");

        let write_error = ImagodError::new(ErrorCode::Internal, "session.write", "stream closed");
        let result = finalize_operation_after_terminal_event(
            &operations,
            &request_id,
            CommandState::Failed,
            "failed",
            Err(write_error),
        )
        .await;

        assert!(result.is_err());
        let snapshot = operations.snapshot_running(&request_id).await;
        assert!(
            snapshot.is_err(),
            "operation should be removed after finalize"
        );
    }

    #[test]
    fn fixed_log_chunk_size_is_bounded_by_target_and_datagram_size() {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let max_datagram_size = 1413usize;
        let chunk_size = fixed_log_chunk_size(
            request_id,
            correlation_id,
            max_datagram_size,
            &["svc-a".to_string()],
        )
        .expect("chunk size should be computed");
        assert!(chunk_size <= 1024);
        assert!(chunk_size > 0);

        let probe = ProtocolEnvelope::new(
            MessageType::LogsChunk,
            request_id,
            correlation_id,
            LogChunk {
                request_id,
                seq: u64::MAX,
                process_id: "svc-a".to_string(),
                stream_kind: LogStreamKind::Composite,
                bytes: vec![0xAB; chunk_size],
                is_last: false,
            },
        );
        let encoded = to_cbor(&probe).expect("encoding must succeed");
        assert!(encoded.len() <= max_datagram_size);
    }

    #[test]
    fn fixed_log_chunk_size_handles_long_process_id() {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let process_id = "service-with-very-long-name-".repeat(12);
        let max_datagram_size = 1413usize;
        let chunk_size = fixed_log_chunk_size(
            request_id,
            correlation_id,
            max_datagram_size,
            std::slice::from_ref(&process_id),
        )
        .expect("chunk size should be computed");
        assert!(chunk_size > 0);

        let probe = ProtocolEnvelope::new(
            MessageType::LogsChunk,
            request_id,
            correlation_id,
            LogChunk {
                request_id,
                seq: u64::MAX,
                process_id,
                stream_kind: LogStreamKind::Composite,
                bytes: vec![0xCD; chunk_size],
                is_last: false,
            },
        );
        let encoded = to_cbor(&probe).expect("encoding must succeed");
        assert!(encoded.len() <= max_datagram_size);
    }

    #[test]
    fn fixed_log_chunk_size_rejects_too_small_datagram() {
        let err = fixed_log_chunk_size(Uuid::new_v4(), Uuid::new_v4(), 8, &["svc-a".to_string()])
            .expect_err("small datagram should be rejected");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "logs.datagram");
    }

    #[test]
    fn log_error_mapping_preserves_common_error_kinds() {
        let not_found = ImagodError::new(ErrorCode::NotFound, "logs", "missing service");
        let mapped_not_found = log_error_from_imagod_error(&not_found);
        assert_eq!(mapped_not_found.code, LogErrorCode::ProcessNotFound);

        let unauthorized = ImagodError::new(ErrorCode::Unauthorized, "logs", "denied");
        let mapped_unauthorized = log_error_from_imagod_error(&unauthorized);
        assert_eq!(mapped_unauthorized.code, LogErrorCode::PermissionDenied);
    }

    #[test]
    fn service_log_stream_maps_to_protocol_stream_kind() {
        assert_eq!(
            service_log_stream_to_protocol(ServiceLogStream::Stdout),
            LogStreamKind::Stdout
        );
        assert_eq!(
            service_log_stream_to_protocol(ServiceLogStream::Stderr),
            LogStreamKind::Stderr
        );
    }
}
