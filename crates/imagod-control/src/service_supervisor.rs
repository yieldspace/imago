//! Service process supervisor and manager-side control-plane server.

use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{
    ControlRequest, ControlResponse, IpcErrorPayload, RunnerBootstrap, RunnerInboundRequest,
    ServiceBinding, compute_manager_auth_proof, dbus_p2p::DbusP2pTransport, issue_invocation_token,
    now_unix_secs, random_secret_hex,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    process::{Child, Command},
    sync::{Mutex, RwLock, oneshot, oneshot::error::TryRecvError},
    time,
};

const STAGE_START: &str = "service.start";
const STAGE_STOP: &str = "service.stop";
const STAGE_CONTROL: &str = "service.control";
const STARTUP_PROBE_POLL_INTERVAL_MS: u64 = 25;
const INVOCATION_TOKEN_TTL_SECS: u64 = 30;
type PendingReadyMap = BTreeMap<String, oneshot::Sender<Result<(), ImagodError>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Launch specification used to spawn one runner process.
pub struct ServiceLaunch {
    /// Service name.
    pub name: String,
    /// Release hash to execute.
    pub release_hash: String,
    /// Component file path.
    pub component_path: PathBuf,
    /// WASI CLI arguments.
    pub args: Vec<String>,
    /// Environment variables for runtime.
    pub envs: BTreeMap<String, String>,
    /// Allowed invocation bindings for this service.
    pub bindings: Vec<ServiceBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Runtime status tracked for each supervised service.
pub enum RunningStatus {
    /// Service is running.
    Running,
    /// Service is being stopped.
    Stopping,
}

#[derive(Debug)]
/// Internal mutable state for one supervised runner process.
struct RunningService {
    release_hash: String,
    started_at: String,
    status: RunningStatus,
    runner_id: String,
    runner_endpoint: PathBuf,
    manager_auth_secret: String,
    invocation_secret: String,
    bindings: Vec<ServiceBinding>,
    child: Child,
    _stdout_log: Arc<Mutex<BoundedLogBuffer>>,
    _stderr_log: Arc<Mutex<BoundedLogBuffer>>,
    last_heartbeat_at: String,
}

#[derive(Debug)]
/// Bounded byte ring used for per-stream runner log capture.
struct BoundedLogBuffer {
    max_bytes: usize,
    bytes: VecDeque<u8>,
}

impl BoundedLogBuffer {
    /// Creates a new bounded log buffer.
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.max(1),
            bytes: VecDeque::new(),
        }
    }

    /// Appends bytes and evicts oldest data when capacity is exceeded.
    fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.bytes.extend(chunk.iter().copied());
        while self.bytes.len() > self.max_bytes {
            let _ = self.bytes.pop_front();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Clone)]
/// Supervises service runner processes and manager-runner control traffic.
pub struct ServiceSupervisor {
    storage_root: PathBuf,
    stop_grace_timeout: Duration,
    runner_ready_timeout: Duration,
    runner_log_buffer_bytes: usize,
    epoch_tick_interval_ms: u64,
    manager_control_endpoint: PathBuf,
    inner: Arc<RwLock<BTreeMap<String, RunningService>>>,
    pending_ready: Arc<Mutex<PendingReadyMap>>,
    stopping_count: Arc<AtomicUsize>,
}

impl ServiceSupervisor {
    /// Creates a service supervisor and starts manager control socket server.
    pub fn new(
        storage_root: impl AsRef<Path>,
        stop_grace_timeout_secs: u64,
        runner_ready_timeout_secs: u64,
        runner_log_buffer_bytes: usize,
        epoch_tick_interval_ms: u64,
    ) -> Result<Self, ImagodError> {
        let storage_root = storage_root.as_ref().to_path_buf();
        let runtime_root = storage_root.join("runtime").join("ipc");
        std::fs::create_dir_all(&runtime_root).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_CONTROL,
                format!(
                    "failed to create runtime dir {}: {e}",
                    runtime_root.display()
                ),
            )
        })?;
        let manager_control_endpoint = runtime_root.join("manager-control.sock");
        if manager_control_endpoint.exists() {
            std::fs::remove_file(&manager_control_endpoint).map_err(|e| {
                ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_CONTROL,
                    format!(
                        "failed to remove stale manager control endpoint {}: {e}",
                        manager_control_endpoint.display()
                    ),
                )
            })?;
        }

        let listener = UnixListener::bind(&manager_control_endpoint).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_CONTROL,
                format!(
                    "failed to bind manager control endpoint {}: {e}",
                    manager_control_endpoint.display()
                ),
            )
        })?;

        let supervisor = Self {
            storage_root,
            stop_grace_timeout: Duration::from_secs(stop_grace_timeout_secs.max(1)),
            runner_ready_timeout: Duration::from_secs(runner_ready_timeout_secs.max(1)),
            runner_log_buffer_bytes: runner_log_buffer_bytes.max(1024),
            epoch_tick_interval_ms: epoch_tick_interval_ms.max(1),
            manager_control_endpoint,
            inner: Arc::new(RwLock::new(BTreeMap::new())),
            pending_ready: Arc::new(Mutex::new(BTreeMap::new())),
            stopping_count: Arc::new(AtomicUsize::new(0)),
        };
        supervisor.spawn_manager_control_server(listener);
        Ok(supervisor)
    }

    /// Starts a service by spawning a runner child process.
    pub async fn start(&self, launch: ServiceLaunch) -> Result<(), ImagodError> {
        self.reap_finished_service(&launch.name).await;

        {
            let inner = self.inner.read().await;
            if inner.contains_key(&launch.name) {
                return Err(ImagodError::new(
                    ErrorCode::Busy,
                    STAGE_START,
                    format!("service '{}' is already running", launch.name),
                ));
            }
        }

        let runner_id = uuid::Uuid::new_v4().to_string();
        let manager_auth_secret = random_secret_hex();
        let invocation_secret = random_secret_hex();
        let runner_endpoint = self.runner_endpoint_for(&launch.name, &runner_id);

        let bootstrap = RunnerBootstrap {
            runner_id: runner_id.clone(),
            service_name: launch.name.clone(),
            release_hash: launch.release_hash.clone(),
            component_path: launch.component_path.clone(),
            args: launch.args.clone(),
            envs: launch.envs.clone(),
            bindings: launch.bindings.clone(),
            manager_control_endpoint: self.manager_control_endpoint.clone(),
            runner_endpoint: runner_endpoint.clone(),
            manager_auth_secret: manager_auth_secret.clone(),
            invocation_secret: invocation_secret.clone(),
            epoch_tick_interval_ms: self.epoch_tick_interval_ms,
        };

        let (ready_tx, mut ready_rx) = oneshot::channel::<Result<(), ImagodError>>();
        self.pending_ready
            .lock()
            .await
            .insert(runner_id.clone(), ready_tx);

        let mut child = match self.spawn_runner_child(&bootstrap) {
            Ok(child) => child,
            Err(err) => {
                self.pending_ready.lock().await.remove(&runner_id);
                return Err(err);
            }
        };

        let stdout_log = Arc::new(Mutex::new(BoundedLogBuffer::new(
            self.runner_log_buffer_bytes / 2,
        )));
        let stderr_log = Arc::new(Mutex::new(BoundedLogBuffer::new(
            self.runner_log_buffer_bytes / 2,
        )));

        if let Some(stdout) = child.stdout.take() {
            spawn_log_drain(stdout, stdout_log.clone(), launch.name.clone(), "stdout");
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_drain(stderr, stderr_log.clone(), launch.name.clone(), "stderr");
        }

        {
            let mut inner = self.inner.write().await;
            inner.insert(
                launch.name.clone(),
                RunningService {
                    release_hash: launch.release_hash,
                    started_at: now_unix_secs().to_string(),
                    status: RunningStatus::Running,
                    runner_id: runner_id.clone(),
                    runner_endpoint,
                    manager_auth_secret,
                    invocation_secret,
                    bindings: launch.bindings,
                    child,
                    _stdout_log: stdout_log,
                    _stderr_log: stderr_log,
                    last_heartbeat_at: now_unix_secs().to_string(),
                },
            );
        }

        if let Err(err) = self
            .write_bootstrap_to_running_service(&launch.name, &bootstrap)
            .await
        {
            self.pending_ready.lock().await.remove(&runner_id);
            self.cleanup_start_failure(&launch.name).await;
            return Err(err);
        }

        let ready_result = self
            .wait_for_runner_ready(&launch.name, &runner_id, &mut ready_rx)
            .await;
        self.pending_ready.lock().await.remove(&runner_id);

        if let Err(err) = ready_result {
            self.cleanup_start_failure(&launch.name).await;
            return Err(err);
        }

        Ok(())
    }

    /// Replaces an existing service using stop-then-start sequence.
    pub async fn replace(&self, launch: ServiceLaunch) -> Result<(), ImagodError> {
        match self.stop(&launch.name, false).await {
            Ok(()) => {}
            Err(err) if err.code == ErrorCode::NotFound => {}
            Err(err) => return Err(err),
        }
        self.start(launch).await
    }

    /// Stops a running service, optionally forcing immediate kill.
    pub async fn stop(&self, service_name: &str, force: bool) -> Result<(), ImagodError> {
        let _stopping_guard = StoppingCounterGuard::new(self.stopping_count.clone());
        let mut service = self.take_running(service_name).await?;

        if let Ok(Some(exit_status)) = service.child.try_wait() {
            log_exit_outcome(
                service_name,
                &service.release_hash,
                &service.started_at,
                service.status,
                exit_status,
            );
            return Err(ImagodError::new(
                ErrorCode::NotFound,
                STAGE_STOP,
                format!("service '{service_name}' is not running"),
            ));
        }

        service.status = RunningStatus::Stopping;

        if force {
            kill_and_wait(&mut service.child).await?;
            return Ok(());
        }

        let shutdown_response = DbusP2pTransport::call_runner(
            &service.runner_endpoint,
            &RunnerInboundRequest::ShutdownRunner,
        )
        .await;
        if let Err(err) = shutdown_response {
            eprintln!(
                "service graceful shutdown request failed name={} release={} error={}",
                service_name, service.release_hash, err
            );
        }

        match time::timeout(self.stop_grace_timeout, service.child.wait()).await {
            Ok(wait_result) => {
                let status = wait_result.map_err(|e| {
                    ImagodError::new(
                        ErrorCode::Internal,
                        STAGE_STOP,
                        format!("runner wait failed: {e}"),
                    )
                })?;
                log_exit_outcome(
                    service_name,
                    &service.release_hash,
                    &service.started_at,
                    service.status,
                    status,
                );
                Ok(())
            }
            Err(_) => {
                kill_and_wait(&mut service.child).await?;
                Ok(())
            }
        }
    }

    /// Reaps all finished services and logs exit outcomes.
    pub async fn reap_finished(&self) {
        let mut finished = Vec::new();
        {
            let mut inner = self.inner.write().await;
            let names = inner.keys().cloned().collect::<Vec<_>>();
            for name in names {
                let status = match inner.get_mut(&name) {
                    Some(service) => match service.child.try_wait() {
                        Ok(Some(status)) => Some(status),
                        Ok(None) => None,
                        Err(err) => {
                            eprintln!(
                                "service try_wait failed name={} release={} error={}",
                                name, service.release_hash, err
                            );
                            None
                        }
                    },
                    None => None,
                };
                if let Some(exit_status) = status
                    && let Some(service) = inner.remove(&name)
                {
                    finished.push((name, service, exit_status));
                }
            }
        }

        for (name, service, status) in finished {
            log_exit_outcome(
                &name,
                &service.release_hash,
                &service.started_at,
                service.status,
                status,
            );
        }
    }

    /// Returns true if at least one service is running or stopping.
    pub async fn has_live_services(&self) -> bool {
        if self.stopping_count.load(Ordering::SeqCst) > 0 {
            return true;
        }
        let inner = self.inner.read().await;
        !inner.is_empty()
    }

    /// Spawns the async manager control server loop on the provided listener.
    fn spawn_manager_control_server(&self, listener: UnixListener) {
        let inner = self.inner.clone();
        let pending_ready = self.pending_ready.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(err) => {
                        eprintln!("manager control accept failed: {err}");
                        continue;
                    }
                };

                let request =
                    match DbusP2pTransport::read_message::<ControlRequest>(&mut stream).await {
                        Ok(v) => v,
                        Err(err) => {
                            let _ = DbusP2pTransport::write_message(
                                &mut stream,
                                &ControlResponse::Error(IpcErrorPayload::from_error(&err)),
                            )
                            .await;
                            continue;
                        }
                    };

                let response = handle_control_request(&inner, &pending_ready, request).await;
                let _ = DbusP2pTransport::write_message(&mut stream, &response).await;
            }
        });
    }

    /// Spawns the `imagod --runner` child process with piped stdio.
    fn spawn_runner_child(&self, _bootstrap: &RunnerBootstrap) -> Result<Child, ImagodError> {
        let exe = std::env::current_exe().map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!("failed to resolve current executable: {e}"),
            )
        })?;
        let mut cmd = Command::new(exe);
        cmd.arg("--runner");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.spawn().map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!("failed to spawn runner process: {e}"),
            )
        })
    }

    fn runner_endpoint_for(&self, service_name: &str, runner_id: &str) -> PathBuf {
        self.storage_root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join(format!("{}-{}.sock", service_name, runner_id))
    }

    /// Waits until runner-ready arrives, child exits, or timeout elapses.
    async fn wait_for_runner_ready(
        &self,
        service_name: &str,
        runner_id: &str,
        ready_rx: &mut oneshot::Receiver<Result<(), ImagodError>>,
    ) -> Result<(), ImagodError> {
        let deadline = time::Instant::now() + self.runner_ready_timeout;
        loop {
            match ready_rx.try_recv() {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(err)) => return Err(err),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Closed) => {
                    return Err(ImagodError::new(
                        ErrorCode::Internal,
                        STAGE_START,
                        format!("runner '{runner_id}' readiness channel closed unexpectedly"),
                    ));
                }
            }

            let exited = {
                let mut inner = self.inner.write().await;
                match inner.get_mut(service_name) {
                    Some(service) => matches!(service.child.try_wait(), Ok(Some(_))),
                    None => true,
                }
            };
            if exited {
                return Err(ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_START,
                    format!("service '{service_name}' exited before ready"),
                ));
            }

            let now = time::Instant::now();
            if now >= deadline {
                return Err(ImagodError::new(
                    ErrorCode::OperationTimeout,
                    STAGE_START,
                    format!("service '{service_name}' did not send runner_ready in time"),
                ));
            }
            let sleep_for = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(STARTUP_PROBE_POLL_INTERVAL_MS));
            time::sleep(sleep_for).await;
        }
    }

    async fn cleanup_start_failure(&self, service_name: &str) {
        let service = {
            let mut inner = self.inner.write().await;
            inner.remove(service_name)
        };
        if let Some(mut service) = service {
            let _ = service.child.start_kill();
            let _ = service.child.wait().await;
        }
    }

    async fn write_bootstrap_to_running_service(
        &self,
        service_name: &str,
        bootstrap: &RunnerBootstrap,
    ) -> Result<(), ImagodError> {
        let bytes = imago_protocol::to_cbor(bootstrap).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!("failed to encode runner bootstrap: {e}"),
            )
        })?;

        let mut stdin = {
            let mut inner = self.inner.write().await;
            let service = inner.get_mut(service_name).ok_or_else(|| {
                ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_START,
                    format!("service '{service_name}' disappeared before bootstrap write"),
                )
            })?;
            service.child.stdin.take().ok_or_else(|| {
                ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_START,
                    "runner stdin is not available",
                )
            })?
        };

        stdin.write_all(&bytes).await.map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!("failed to write runner bootstrap: {e}"),
            )
        })?;
        stdin.shutdown().await.map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!("failed to close runner bootstrap stdin: {e}"),
            )
        })
    }

    async fn reap_finished_service(&self, service_name: &str) {
        let should_reap = {
            let mut inner = self.inner.write().await;
            match inner.get_mut(service_name) {
                Some(service) => match service.child.try_wait() {
                    Ok(Some(status)) => {
                        let service = inner.remove(service_name);
                        if let Some(service) = service {
                            log_exit_outcome(
                                service_name,
                                &service.release_hash,
                                &service.started_at,
                                service.status,
                                status,
                            );
                        }
                        true
                    }
                    Ok(None) => false,
                    Err(err) => {
                        eprintln!(
                            "service try_wait failed name={} release={} error={}",
                            service_name, service.release_hash, err
                        );
                        false
                    }
                },
                None => false,
            }
        };

        if should_reap {
            self.pending_ready
                .lock()
                .await
                .retain(|_, sender| !sender.is_closed());
        }
    }

    async fn take_running(&self, service_name: &str) -> Result<RunningService, ImagodError> {
        let service = {
            let mut inner = self.inner.write().await;
            inner.remove(service_name)
        };
        service.ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                STAGE_STOP,
                format!("service '{service_name}' is not running"),
            )
        })
    }
}

/// Handles one control request received on manager control socket.
async fn handle_control_request(
    inner: &Arc<RwLock<BTreeMap<String, RunningService>>>,
    pending_ready: &Arc<Mutex<PendingReadyMap>>,
    request: ControlRequest,
) -> ControlResponse {
    match request {
        ControlRequest::RegisterRunner {
            runner_id,
            service_name,
            release_hash,
            runner_endpoint,
            manager_auth_proof,
        } => {
            let mut guard = inner.write().await;
            let Some((actual_service_name, service)) = guard
                .iter_mut()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "runner is not registered for startup");
            };

            if let Err(err) = validate_manager_auth(
                &service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }

            if actual_service_name != &service_name || service.release_hash != release_hash {
                return control_error(ErrorCode::BadRequest, "register_runner metadata mismatch");
            }

            service.runner_endpoint = runner_endpoint;
            service.last_heartbeat_at = now_unix_secs().to_string();
            ControlResponse::Ack
        }
        ControlRequest::RunnerReady {
            runner_id,
            manager_auth_proof,
        } => {
            {
                let mut guard = inner.write().await;
                let Some((_, service)) = guard
                    .iter_mut()
                    .find(|(_, service)| service.runner_id == runner_id)
                else {
                    return control_error(ErrorCode::NotFound, "runner is not registered");
                };

                if let Err(err) = validate_manager_auth(
                    &service.manager_auth_secret,
                    &runner_id,
                    &manager_auth_proof,
                ) {
                    return ControlResponse::Error(IpcErrorPayload::from_error(&err));
                }

                service.last_heartbeat_at = now_unix_secs().to_string();
            }

            if let Some(sender) = pending_ready.lock().await.remove(&runner_id) {
                let _ = sender.send(Ok(()));
            }
            ControlResponse::Ack
        }
        ControlRequest::Heartbeat {
            runner_id,
            manager_auth_proof,
        } => {
            let mut guard = inner.write().await;
            let Some((_, service)) = guard
                .iter_mut()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "runner is not registered");
            };

            if let Err(err) = validate_manager_auth(
                &service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }

            service.last_heartbeat_at = now_unix_secs().to_string();
            ControlResponse::Ack
        }
        ControlRequest::ResolveInvocationTarget {
            runner_id,
            manager_auth_proof,
            target_service,
            wit,
        } => {
            let guard = inner.read().await;

            let Some((source_service_name, source_service)) = guard
                .iter()
                .find(|(_, service)| service.runner_id == runner_id)
            else {
                return control_error(ErrorCode::NotFound, "source runner is not registered");
            };

            if let Err(err) = validate_manager_auth(
                &source_service.manager_auth_secret,
                &runner_id,
                &manager_auth_proof,
            ) {
                return ControlResponse::Error(IpcErrorPayload::from_error(&err));
            }

            if !is_binding_allowed(&source_service.bindings, &target_service, &wit) {
                return control_error(
                    ErrorCode::Unauthorized,
                    "binding does not allow target service/interface",
                );
            }

            let Some(target_runner) = guard.get(&target_service) else {
                return control_error(ErrorCode::NotFound, "target service is not running");
            };

            let claims = imagod_ipc::InvocationTokenClaims {
                source_service: source_service_name.clone(),
                target_service: target_service.clone(),
                wit: wit.clone(),
                exp: now_unix_secs() + INVOCATION_TOKEN_TTL_SECS,
                nonce: uuid::Uuid::new_v4().to_string(),
            };
            let token = match issue_invocation_token(&target_runner.invocation_secret, claims) {
                Ok(token) => token,
                Err(err) => return ControlResponse::Error(IpcErrorPayload::from_error(&err)),
            };

            ControlResponse::ResolvedInvocationTarget {
                endpoint: target_runner.runner_endpoint.clone(),
                token,
            }
        }
    }
}

/// Validates manager proof generated from shared secret and runner id.
fn validate_manager_auth(secret: &str, runner_id: &str, proof: &str) -> Result<(), ImagodError> {
    let expected = compute_manager_auth_proof(secret, runner_id)?;
    if expected == proof {
        return Ok(());
    }

    Err(ImagodError::new(
        ErrorCode::Unauthorized,
        STAGE_CONTROL,
        "manager auth proof mismatch",
    ))
}

fn control_error(code: ErrorCode, message: impl Into<String>) -> ControlResponse {
    ControlResponse::Error(IpcErrorPayload {
        code,
        stage: STAGE_CONTROL.to_string(),
        message: message.into(),
    })
}

/// Returns whether a binding list allows the target service/interface pair.
fn is_binding_allowed(bindings: &[ServiceBinding], target_service: &str, wit: &str) -> bool {
    bindings
        .iter()
        .any(|binding| binding.target == target_service && binding.wit == wit)
}

/// Drains one child output stream into bounded in-memory log buffer.
///
/// Concurrency: runs as a detached task per stream.
fn spawn_log_drain<R>(
    mut reader: R,
    buffer: Arc<Mutex<BoundedLogBuffer>>,
    service_name: String,
    stream_name: &'static str,
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

/// Sends kill signal to child and waits for termination.
async fn kill_and_wait(child: &mut Child) -> Result<(), ImagodError> {
    child.start_kill().map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_STOP,
            format!("failed to signal runner kill: {e}"),
        )
    })?;
    let _ = child.wait().await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_STOP,
            format!("failed to wait killed runner: {e}"),
        )
    })?;
    Ok(())
}

fn log_exit_outcome(
    service_name: &str,
    release_hash: &str,
    started_at: &str,
    status: RunningStatus,
    exit_status: ExitStatus,
) {
    eprintln!(
        "service stopped name={} release={} started_at={} state={:?} exit_status={}",
        service_name, release_hash, started_at, status, exit_status
    );
}

/// RAII guard that tracks number of concurrent stop operations.
struct StoppingCounterGuard {
    counter: Arc<AtomicUsize>,
}

impl StoppingCounterGuard {
    /// Increments stop counter and returns a guard that decrements on drop.
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for StoppingCounterGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_log_buffer_keeps_latest_bytes_only() {
        let mut buffer = BoundedLogBuffer::new(5);
        buffer.push(b"abc");
        buffer.push(b"def");
        assert_eq!(buffer.len(), 5);
    }

    #[test]
    fn bindings_allow_target_and_wit_pair_only() {
        let bindings = vec![ServiceBinding {
            target: "svc-b".to_string(),
            wit: "pkg:iface/callable".to_string(),
        }];
        assert!(is_binding_allowed(&bindings, "svc-b", "pkg:iface/callable"));
        assert!(!is_binding_allowed(&bindings, "svc-b", "pkg:iface/other"));
        assert!(!is_binding_allowed(
            &bindings,
            "svc-c",
            "pkg:iface/callable"
        ));
    }

    #[test]
    fn manager_auth_validation_rejects_wrong_proof() {
        let secret = random_secret_hex();
        let err = validate_manager_auth(&secret, "runner-1", "invalid-proof")
            .expect_err("proof validation should fail");
        assert_eq!(err.code, ErrorCode::Unauthorized);
    }
}
