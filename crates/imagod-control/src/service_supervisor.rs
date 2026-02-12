//! Service process supervisor and manager-side control-plane server.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    process::{Child, Command},
    sync::{Mutex, RwLock, oneshot, oneshot::error::TryRecvError, watch},
    task::{JoinHandle, JoinSet},
    time,
};

const STAGE_START: &str = "service.start";
const STAGE_STOP: &str = "service.stop";
const STAGE_CONTROL: &str = "service.control";
const STARTUP_PROBE_POLL_INTERVAL_MS: u64 = 25;
const INVOCATION_TOKEN_TTL_SECS: u64 = 30;
const RUNNER_ENDPOINT_HASH_BYTES: usize = 16;
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

#[derive(Debug)]
struct ManagerControlServer {
    endpoint: PathBuf,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Drop for ManagerControlServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        self.task.abort();
        remove_manager_control_endpoint_best_effort(&self.endpoint);
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
    starting_services: Arc<Mutex<BTreeSet<String>>>,
    stopping_count: Arc<AtomicUsize>,
    _manager_control_server: Arc<ManagerControlServer>,
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
        let stop_grace_timeout = Duration::from_secs(stop_grace_timeout_secs.max(1));
        let runner_ready_timeout = Duration::from_secs(runner_ready_timeout_secs.max(1));

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

        let inner = Arc::new(RwLock::new(BTreeMap::new()));
        let pending_ready = Arc::new(Mutex::new(BTreeMap::new()));
        let starting_services = Arc::new(Mutex::new(BTreeSet::new()));
        let stopping_count = Arc::new(AtomicUsize::new(0));
        let manager_control_server = Arc::new(Self::spawn_manager_control_server(
            listener,
            manager_control_endpoint.clone(),
            inner.clone(),
            pending_ready.clone(),
            runner_ready_timeout,
        ));

        let supervisor = Self {
            storage_root,
            stop_grace_timeout,
            runner_ready_timeout,
            runner_log_buffer_bytes: runner_log_buffer_bytes.max(1024),
            epoch_tick_interval_ms: epoch_tick_interval_ms.max(1),
            manager_control_endpoint,
            inner,
            pending_ready,
            starting_services,
            stopping_count,
            _manager_control_server: manager_control_server,
        };
        Ok(supervisor)
    }

    /// Starts a service by spawning a runner child process.
    pub async fn start(&self, launch: ServiceLaunch) -> Result<(), ImagodError> {
        self.reap_finished_service(&launch.name).await;
        self.reserve_start(&launch.name).await?;
        let service_name = launch.name.clone();
        let result = async {
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
        .await;
        self.release_start(&service_name).await;
        result
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
        let _runner_endpoint_cleanup =
            RunnerEndpointCleanupGuard::new(service.runner_endpoint.clone());

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

        let stop_deadline = time::Instant::now() + self.stop_grace_timeout;
        let shutdown_timeout = self.runner_ready_timeout.min(self.stop_grace_timeout);

        match compute_manager_auth_proof(&service.manager_auth_secret, &service.runner_id) {
            Ok(manager_auth_proof) => {
                match time::timeout(
                    shutdown_timeout,
                    DbusP2pTransport::call_runner(
                        &service.runner_endpoint,
                        &RunnerInboundRequest::ShutdownRunner { manager_auth_proof },
                    ),
                )
                .await
                {
                    Ok(Ok(_response)) => {}
                    Ok(Err(err)) => {
                        eprintln!(
                            "service graceful shutdown request failed name={} release={} error={}",
                            service_name, service.release_hash, err
                        );
                    }
                    Err(_) => {
                        eprintln!(
                            "service graceful shutdown request timed out name={} release={} timeout_ms={}",
                            service_name,
                            service.release_hash,
                            shutdown_timeout.as_millis()
                        );
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "service graceful shutdown auth proof failed name={} release={} error={}",
                    service_name, service.release_hash, err
                );
            }
        }

        let remaining = stop_deadline.saturating_duration_since(time::Instant::now());
        if remaining.is_zero() {
            kill_and_wait(&mut service.child).await?;
            return Ok(());
        }

        match time::timeout(remaining, service.child.wait()).await {
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

    /// Stops all currently tracked services in parallel.
    pub async fn stop_all(&self, force: bool) -> Vec<(String, ImagodError)> {
        let service_names = {
            let inner = self.inner.read().await;
            inner.keys().cloned().collect::<Vec<_>>()
        };

        let mut join_set = JoinSet::new();
        for service_name in service_names {
            let supervisor = self.clone();
            join_set.spawn(async move {
                let result = supervisor.stop(&service_name, force).await;
                (service_name, result)
            });
        }

        let mut errors = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((_service_name, Ok(()))) => {}
                Ok((_service_name, Err(err))) if err.code == ErrorCode::NotFound => {}
                Ok((service_name, Err(err))) => errors.push((service_name, err)),
                Err(err) => errors.push((
                    "<unknown>".to_string(),
                    ImagodError::new(
                        ErrorCode::Internal,
                        STAGE_STOP,
                        format!("stop_all task join failed: {err}"),
                    ),
                )),
            }
        }
        errors
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
            remove_runner_endpoint_best_effort(&service.runner_endpoint);
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
    fn spawn_manager_control_server(
        listener: UnixListener,
        endpoint: PathBuf,
        inner: Arc<RwLock<BTreeMap<String, RunningService>>>,
        pending_ready: Arc<Mutex<PendingReadyMap>>,
        read_timeout: Duration,
    ) -> ManagerControlServer {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        let (stream, _) = match accepted {
                            Ok(v) => v,
                            Err(err) => {
                                eprintln!("manager control accept failed: {err}");
                                continue;
                            }
                        };

                        let inner = inner.clone();
                        let pending_ready = pending_ready.clone();
                        tokio::spawn(async move {
                            handle_control_connection(stream, inner, pending_ready, read_timeout).await;
                        });
                    }
                }
            }
        });

        ManagerControlServer {
            endpoint,
            shutdown_tx,
            task,
        }
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
        build_runner_endpoint(&self.storage_root, service_name, runner_id)
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

    async fn reserve_start(&self, service_name: &str) -> Result<(), ImagodError> {
        {
            let mut starting_services = self.starting_services.lock().await;
            if starting_services.contains(service_name) {
                return Err(start_busy_error(service_name));
            }
            starting_services.insert(service_name.to_string());
        }

        let already_running = {
            let inner = self.inner.read().await;
            inner.contains_key(service_name)
        };
        if already_running {
            self.release_start(service_name).await;
            return Err(start_busy_error(service_name));
        }
        Ok(())
    }

    async fn release_start(&self, service_name: &str) {
        self.starting_services.lock().await.remove(service_name);
    }

    async fn cleanup_start_failure(&self, service_name: &str) {
        let service = {
            let mut inner = self.inner.write().await;
            inner.remove(service_name)
        };
        if let Some(mut service) = service {
            let _ = service.child.start_kill();
            let _ = service.child.wait().await;
            remove_runner_endpoint_best_effort(&service.runner_endpoint);
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
                            remove_runner_endpoint_best_effort(&service.runner_endpoint);
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

fn build_runner_endpoint(storage_root: &Path, service_name: &str, runner_id: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(service_name.as_bytes());
    hasher.update(b":");
    hasher.update(runner_id.as_bytes());
    let digest = hasher.finalize();
    let endpoint_hash = hex::encode(&digest[..RUNNER_ENDPOINT_HASH_BYTES]);

    storage_root
        .join("runtime")
        .join("ipc")
        .join("runners")
        .join(format!("runner-{endpoint_hash}.sock"))
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

            if service.runner_endpoint != runner_endpoint {
                return control_error(ErrorCode::BadRequest, "register_runner endpoint mismatch");
            }
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

async fn handle_control_connection(
    mut stream: UnixStream,
    inner: Arc<RwLock<BTreeMap<String, RunningService>>>,
    pending_ready: Arc<Mutex<PendingReadyMap>>,
    read_timeout: Duration,
) {
    let request = match time::timeout(
        read_timeout,
        DbusP2pTransport::read_message::<ControlRequest>(&mut stream),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(err)) => {
            let _ = DbusP2pTransport::write_message(
                &mut stream,
                &ControlResponse::Error(IpcErrorPayload::from_error(&err)),
            )
            .await;
            return;
        }
        Err(_) => {
            let timeout_error = IpcErrorPayload {
                code: ErrorCode::OperationTimeout,
                stage: STAGE_CONTROL.to_string(),
                message: format!(
                    "manager control request read timed out after {} ms",
                    read_timeout.as_millis()
                ),
            };
            if let Err(err) =
                DbusP2pTransport::write_message(&mut stream, &ControlResponse::Error(timeout_error))
                    .await
            {
                eprintln!("manager control timeout response write failed: {err}");
            }
            return;
        }
    };

    let response = handle_control_request(&inner, &pending_ready, request).await;
    let _ = DbusP2pTransport::write_message(&mut stream, &response).await;
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
    if let Err(err) = child.start_kill() {
        return match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_STOP,
                format!("failed to signal runner kill: {err}"),
            )),
            Err(wait_err) => Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_STOP,
                format!(
                    "failed to signal runner kill: {err}; failed to check child state: {wait_err}"
                ),
            )),
        };
    }
    let _ = child.wait().await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_STOP,
            format!("failed to wait killed runner: {e}"),
        )
    })?;
    Ok(())
}

fn remove_socket_best_effort(path: &Path, socket_name: &str) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("failed to remove {socket_name} {}: {err}", path.display());
        }
        Err(_) => {}
    }
}

fn remove_runner_endpoint_best_effort(path: &Path) {
    remove_socket_best_effort(path, "runner endpoint");
}

fn remove_manager_control_endpoint_best_effort(path: &Path) {
    remove_socket_best_effort(path, "manager control endpoint");
}

fn start_busy_error(service_name: &str) -> ImagodError {
    ImagodError::new(
        ErrorCode::Busy,
        STAGE_START,
        format!("service '{service_name}' is already running"),
    )
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

struct RunnerEndpointCleanupGuard {
    path: PathBuf,
}

impl RunnerEndpointCleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for RunnerEndpointCleanupGuard {
    fn drop(&mut self) {
        remove_runner_endpoint_best_effort(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::{net::UnixListener, process::Command};

    fn new_test_root(prefix: &str) -> PathBuf {
        let _ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
        PathBuf::from(format!("/tmp/iss-{prefix}-{id}"))
    }

    fn new_running_service(
        child: Child,
        runner_id: &str,
        runner_endpoint: PathBuf,
    ) -> RunningService {
        RunningService {
            release_hash: "release-a".to_string(),
            started_at: now_unix_secs().to_string(),
            status: RunningStatus::Running,
            runner_id: runner_id.to_string(),
            runner_endpoint,
            manager_auth_secret: random_secret_hex(),
            invocation_secret: random_secret_hex(),
            bindings: Vec::new(),
            child,
            _stdout_log: Arc::new(Mutex::new(BoundedLogBuffer::new(64))),
            _stderr_log: Arc::new(Mutex::new(BoundedLogBuffer::new(64))),
            last_heartbeat_at: now_unix_secs().to_string(),
        }
    }

    #[test]
    fn bounded_log_buffer_keeps_latest_bytes_only() {
        let mut buffer = BoundedLogBuffer::new(5);
        buffer.push(b"abc");
        buffer.push(b"def");
        assert_eq!(buffer.len(), 5);
    }

    #[tokio::test]
    async fn runner_endpoint_for_uses_fixed_length_hash_name() {
        let root = new_test_root("endpoint-hash");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");

        let long_service_name = "svc-".to_string() + &"x".repeat(200);
        let endpoint = supervisor.runner_endpoint_for(&long_service_name, "runner-1");
        let file_name = endpoint
            .file_name()
            .and_then(|name| name.to_str())
            .expect("runner endpoint should have UTF-8 file name");

        assert!(file_name.starts_with("runner-"));
        assert!(file_name.ends_with(".sock"));
        assert_eq!(
            file_name.len(),
            "runner-".len() + (RUNNER_ENDPOINT_HASH_BYTES * 2) + ".sock".len()
        );

        drop(supervisor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn runner_endpoint_path_fits_unix_limit_for_var_lib_imago() {
        let long_service_name = "svc-".to_string() + &"x".repeat(200);
        let endpoint = build_runner_endpoint(Path::new("/var/lib/imago"), &long_service_name, "r1");
        let path_len = endpoint.to_string_lossy().len();
        assert!(
            path_len <= 107,
            "runner endpoint path must fit AF_UNIX limit: {path_len}"
        );
    }

    #[tokio::test]
    async fn reserve_start_rejects_duplicate_service_name() {
        let root = new_test_root("start-reserve");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");

        supervisor
            .reserve_start("svc-reserve")
            .await
            .expect("first reservation should succeed");
        let err = supervisor
            .reserve_start("svc-reserve")
            .await
            .expect_err("second reservation should fail");
        assert_eq!(err.code, ErrorCode::Busy);

        supervisor.release_start("svc-reserve").await;
        let _ = std::fs::remove_dir_all(&root);
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

    #[tokio::test]
    async fn kill_and_wait_succeeds_when_child_already_exited() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child process should spawn");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = kill_and_wait(&mut child).await;
        assert!(
            result.is_ok(),
            "already exited child should be treated as stopped"
        );
    }

    #[tokio::test]
    async fn stop_returns_when_shutdown_ipc_hangs() {
        let root = new_test_root("stop-timeout");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");

        let runner_endpoint = root.join("runtime").join("ipc").join("hung-runner.sock");
        if let Some(parent) = runner_endpoint.parent() {
            std::fs::create_dir_all(parent).expect("runner endpoint parent should be created");
        }
        let _ = std::fs::remove_file(&runner_endpoint);
        let listener = UnixListener::bind(&runner_endpoint).expect("runner listener should bind");
        let server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept should succeed");
            let _ = DbusP2pTransport::read_message::<RunnerInboundRequest>(&mut stream).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                "svc-stop-timeout".to_string(),
                new_running_service(child, "runner-stop-timeout", runner_endpoint.clone()),
            );
        }

        let stop_result = tokio::time::timeout(
            Duration::from_secs(3),
            supervisor.stop("svc-stop-timeout", false),
        )
        .await;
        assert!(stop_result.is_ok(), "stop should not hang");
        assert!(
            stop_result.expect("timeout should not elapse").is_ok(),
            "stop should succeed after timeout fallback"
        );

        server_task.abort();
        let _ = std::fs::remove_file(&runner_endpoint);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_force_removes_runner_endpoint() {
        let root = new_test_root("stop-force-cleanup");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");

        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("force-cleanup.sock");
        if let Some(parent) = runner_endpoint.parent() {
            std::fs::create_dir_all(parent).expect("runner endpoint parent should be created");
        }
        std::fs::write(&runner_endpoint, b"stale")
            .expect("runner endpoint fixture should be created");

        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                "svc-force-cleanup".to_string(),
                new_running_service(child, "runner-force-cleanup", runner_endpoint.clone()),
            );
        }

        let stop_result = supervisor.stop("svc-force-cleanup", true).await;
        assert!(stop_result.is_ok(), "force stop should succeed");
        assert!(
            !runner_endpoint.exists(),
            "runner endpoint should be removed after force stop"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_all_stops_multiple_services_in_parallel() {
        let root = new_test_root("stop-all");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");

        let endpoint_a = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-all-a.sock");
        let endpoint_b = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-all-b.sock");
        if let Some(parent) = endpoint_a.parent() {
            std::fs::create_dir_all(parent).expect("runner endpoint parent should be created");
        }

        let child_a = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child A should spawn");
        let child_b = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child B should spawn");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                "svc-stop-all-a".to_string(),
                new_running_service(child_a, "runner-stop-all-a", endpoint_a),
            );
            inner.insert(
                "svc-stop-all-b".to_string(),
                new_running_service(child_b, "runner-stop-all-b", endpoint_b),
            );
        }

        let result = tokio::time::timeout(Duration::from_secs(4), supervisor.stop_all(false))
            .await
            .expect("stop_all should complete");
        assert!(result.is_empty(), "stop_all should have no errors");
        assert!(
            supervisor.inner.read().await.is_empty(),
            "all services should be removed"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_all_ignores_not_found_errors() {
        let root = new_test_root("stop-all-not-found");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");

        let endpoint_done = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-all-done.sock");
        let endpoint_live = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-all-live.sock");
        if let Some(parent) = endpoint_done.parent() {
            std::fs::create_dir_all(parent).expect("runner endpoint parent should be created");
        }

        let done_child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("done child should spawn");
        let live_child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("live child should spawn");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                "svc-stop-all-done".to_string(),
                new_running_service(done_child, "runner-stop-all-done", endpoint_done),
            );
            inner.insert(
                "svc-stop-all-live".to_string(),
                new_running_service(live_child, "runner-stop-all-live", endpoint_live),
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let errors = supervisor.stop_all(true).await;
        assert!(
            errors.is_empty(),
            "NotFound races should be ignored in stop_all"
        );
        assert!(
            supervisor.inner.read().await.is_empty(),
            "all services should be removed"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn wait_for_runner_ready_times_out_without_ready_signal() {
        let root = new_test_root("ready-timeout");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");
        let service_name = "svc-ready-timeout";
        let runner_id = "runner-ready-timeout";
        let runner_endpoint = root.join("runtime").join("ipc").join("ready-timeout.sock");

        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                service_name.to_string(),
                new_running_service(child, runner_id, runner_endpoint),
            );
        }

        let (_ready_tx, mut ready_rx) = oneshot::channel::<Result<(), ImagodError>>();
        let err = tokio::time::timeout(
            Duration::from_secs(3),
            supervisor.wait_for_runner_ready(service_name, runner_id, &mut ready_rx),
        )
        .await
        .expect("runner ready wait should return")
        .expect_err("runner ready wait should timeout");

        assert_eq!(err.code, ErrorCode::OperationTimeout);
        assert!(err.message.contains("did not send runner_ready in time"));

        supervisor.cleanup_start_failure(service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn wait_for_runner_ready_fails_when_child_exits_early() {
        let root = new_test_root("ready-exit");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");
        let service_name = "svc-ready-exit";
        let runner_id = "runner-ready-exit";
        let runner_endpoint = root.join("runtime").join("ipc").join("ready-exit.sock");

        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child process should spawn");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                service_name.to_string(),
                new_running_service(child, runner_id, runner_endpoint),
            );
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_ready_tx, mut ready_rx) = oneshot::channel::<Result<(), ImagodError>>();
        let err = tokio::time::timeout(
            Duration::from_secs(2),
            supervisor.wait_for_runner_ready(service_name, runner_id, &mut ready_rx),
        )
        .await
        .expect("runner ready wait should return")
        .expect_err("runner ready wait should fail when child exits");

        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.message.contains("exited before ready"));

        supervisor.cleanup_start_failure(service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn wait_for_runner_ready_accepts_ready_arriving_at_timeout_boundary() {
        let root = new_test_root("ready-boundary");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");
        let service_name = "svc-ready-boundary".to_string();
        let runner_id = "runner-ready-boundary".to_string();
        let runner_endpoint = root.join("runtime").join("ipc").join("ready-boundary.sock");

        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                service_name.clone(),
                new_running_service(child, &runner_id, runner_endpoint),
            );
        }

        let (ready_tx, mut ready_rx) = oneshot::channel::<Result<(), ImagodError>>();
        let lock_guard = supervisor.inner.write().await;
        let wait_supervisor = supervisor.clone();
        let wait_service_name = service_name.clone();
        let wait_runner_id = runner_id.clone();
        let wait_task = tokio::spawn(async move {
            wait_supervisor
                .wait_for_runner_ready(&wait_service_name, &wait_runner_id, &mut ready_rx)
                .await
        });

        tokio::time::sleep(Duration::from_millis(25)).await;
        ready_tx.send(Ok(())).expect("ready signal should send");
        tokio::time::sleep(Duration::from_millis(1200)).await;
        drop(lock_guard);

        let result = tokio::time::timeout(Duration::from_secs(2), wait_task)
            .await
            .expect("wait task should finish")
            .expect("wait task should not panic");
        assert!(
            result.is_ok(),
            "ready arriving by deadline should avoid false timeout"
        );

        supervisor.cleanup_start_failure(&service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn reap_finished_removes_runner_endpoint() {
        let root = new_test_root("reap-cleanup");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");
        let service_name = "svc-reap-cleanup";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("reap-cleanup.sock");
        if let Some(parent) = runner_endpoint.parent() {
            std::fs::create_dir_all(parent).expect("runner endpoint parent should be created");
        }
        std::fs::write(&runner_endpoint, b"stale")
            .expect("runner endpoint fixture should be created");

        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child process should spawn");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                service_name.to_string(),
                new_running_service(child, "runner-reap-cleanup", runner_endpoint.clone()),
            );
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        supervisor.reap_finished().await;

        assert!(
            !runner_endpoint.exists(),
            "runner endpoint should be removed after reap"
        );
        let inner = supervisor.inner.read().await;
        assert!(
            !inner.contains_key(service_name),
            "finished service should be removed from supervisor map"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn manager_control_idle_connection_does_not_block_next_request() {
        let root = new_test_root("control-parallel");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");

        let idle = tokio::net::UnixStream::connect(&supervisor.manager_control_endpoint)
            .await
            .expect("idle connection should open");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let response = tokio::time::timeout(
            Duration::from_secs(2),
            DbusP2pTransport::call_control(
                &supervisor.manager_control_endpoint,
                &ControlRequest::Heartbeat {
                    runner_id: "missing-runner".to_string(),
                    manager_auth_proof: "proof".to_string(),
                },
            ),
        )
        .await
        .expect("second request should not be blocked")
        .expect("call_control should return response");

        match response {
            ControlResponse::Error(err) => assert_eq!(err.code, ErrorCode::NotFound),
            other => panic!("unexpected response: {other:?}"),
        }

        drop(idle);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn dropping_supervisor_removes_manager_control_endpoint() {
        let root = new_test_root("control-cleanup");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");
        let endpoint = supervisor.manager_control_endpoint.clone();
        assert!(
            endpoint.exists(),
            "manager control endpoint should exist while supervisor is alive"
        );

        drop(supervisor);
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            !endpoint.exists(),
            "manager control endpoint should be cleaned up on drop"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn manager_control_server_stops_accepting_after_drop() {
        let root = new_test_root("control-shutdown");
        let supervisor =
            ServiceSupervisor::new(&root, 1, 1, 4096, 50).expect("supervisor should initialize");
        let endpoint = supervisor.manager_control_endpoint.clone();

        let initial = tokio::net::UnixStream::connect(&endpoint)
            .await
            .expect("initial connection should open");
        drop(initial);

        drop(supervisor);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let reconnect = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::net::UnixStream::connect(&endpoint),
        )
        .await
        .expect("reconnect should complete quickly");
        assert!(
            reconnect.is_err(),
            "manager control server should not accept new connections after drop"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
