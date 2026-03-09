//! Service process supervisor and manager-side control-plane server.
//!
//! Contract highlights:
//! - one runner process per service with readiness gating
//! - bounded in-memory log retention for live subscriptions
//! - bounded file-backed log retention for stopped subscriptions
//! - manager/runner auth proof and invocation token enforcement
//! - graceful stop first, forced termination fallback

#[cfg(test)]
use std::process::Stdio;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use imagod_common::{
    DEFAULT_WASM_GUARD_BEFORE_LINEAR_MEMORY, DEFAULT_WASM_MEMORY_GUARD_SIZE_BYTES,
    DEFAULT_WASM_MEMORY_RESERVATION_BYTES, DEFAULT_WASM_MEMORY_RESERVATION_FOR_GROWTH_BYTES,
    DEFAULT_WASM_PARALLEL_COMPILATION, ImagodError,
};
use imagod_ipc::{
    compute_manager_auth_proof, dbus_p2p::DbusP2pTransport, issue_invocation_token, now_unix_secs,
    random_secret_hex,
};
use imagod_spec::{
    CapabilityPolicy, ErrorCode, InvocationTokenClaims, PluginDependency, ResourceMap,
    RunnerAppType, RunnerBootstrap, RunnerInboundRequest, RunnerInboundResponse,
    RunnerSocketConfig, RunnerWasiMount, ServiceBinding, WasiHttpOutboundRule,
};
#[cfg(test)]
use imagod_spec::{ControlRequest, ControlResponse};
use tokio::{
    io::{AsyncRead, AsyncWriteExt},
    net::UnixListener,
    process::Child,
    sync::{Mutex, RwLock, Semaphore, broadcast, oneshot, oneshot::error::TryRecvError, watch},
    task::{JoinHandle, JoinSet},
    time,
};

use self::{
    log_buffer::CompositeLogBuffer, manager_control::DefaultManagerControlHandler,
    retained_file_store::RetainedFileLogStore, runner_spawn::DefaultRunnerSpawner,
};

#[cfg(feature = "bench-internals")]
pub mod bench_internals;
mod log_buffer;
mod manager_control;
mod remote_rpc;
mod retained_file_store;
mod runner_spawn;

const STAGE_START: &str = "service.start";
const STAGE_STOP: &str = "service.stop";
const STAGE_CONTROL: &str = "service.control";
const STAGE_LOGS: &str = "service.logs";
const STAGE_INVOKE: &str = "service.invoke";
const DETAIL_WASM_STDOUT: &str = "wasm.stdout";
const DETAIL_WASM_STDERR: &str = "wasm.stderr";
const STARTUP_EXIT_CHECK_INTERVAL_MS: u64 = 25;
const INVOCATION_TOKEN_TTL_SECS: u64 = 30;
const RUNNER_ENDPOINT_HASH_BYTES: usize = 16;
const MAX_MANAGER_CONTROL_CONNECTION_HANDLERS: usize = 32;
const MAX_UNIX_SOCKET_PATH_BYTES: usize = 107;
const LOG_CHANNEL_CAPACITY: usize = 256;
const DEFAULT_HTTP_QUEUE_MEMORY_BUDGET_BYTES: u64 = 32 * 1024 * 1024;
type PendingReadyMap = BTreeMap<String, oneshot::Sender<Result<(), ImagodError>>>;
type StoppingServicesMap = BTreeMap<String, usize>;
type RunnerServiceIndex = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Launch specification used to spawn one runner process.
pub struct ServiceLaunch {
    /// Service name.
    pub name: String,
    /// Release hash to execute.
    pub release_hash: String,
    /// Runtime execution model.
    pub app_type: RunnerAppType,
    /// TCP port for HTTP ingress when `app_type=http`.
    pub http_port: Option<u16>,
    /// Max accepted HTTP request body size in bytes when `app_type=http`.
    pub http_max_body_bytes: Option<u64>,
    /// Socket runtime settings when `app_type=socket`.
    pub socket: Option<RunnerSocketConfig>,
    /// Component file path.
    pub component_path: PathBuf,
    /// WASI CLI arguments.
    pub args: Vec<String>,
    /// Environment variables for runtime.
    pub envs: BTreeMap<String, String>,
    /// WASI preopened directory mounts.
    pub wasi_mounts: Vec<RunnerWasiMount>,
    /// Allowed outbound rules for `wasi:http` requests.
    pub wasi_http_outbound: Vec<WasiHttpOutboundRule>,
    /// Arbitrary resource policy map available to runtime/native plugins.
    pub resources: ResourceMap,
    /// Allowed invocation bindings for this service.
    pub bindings: Vec<ServiceBinding>,
    /// Plugin dependencies available to the runtime.
    pub plugin_dependencies: Vec<PluginDependency>,
    /// App-level capability policy.
    pub capabilities: CapabilityPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Runtime status tracked for each supervised service.
pub enum RunningStatus {
    /// Service is running.
    Running,
    /// Service is being stopped.
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Runtime metadata snapshot for one supervised service.
pub struct RuntimeServiceState {
    pub name: String,
    pub release_hash: String,
    pub started_at: String,
    pub status: RunningStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Logical stream of one emitted log event.
pub enum ServiceLogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Incremental log event emitted from one running service.
pub struct ServiceLogEvent {
    pub stream: ServiceLogStream,
    pub bytes: Vec<u8>,
    pub timestamp_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Snapshot payload returned for a logs subscription request.
pub enum ServiceLogSnapshot {
    Bytes(Vec<u8>),
    Events(Vec<ServiceLogEvent>),
}

impl ServiceLogSnapshot {
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            Self::Events(_) => None,
        }
    }

    pub fn as_events(&self) -> Option<&[ServiceLogEvent]> {
        match self {
            Self::Bytes(_) => None,
            Self::Events(events) => Some(events),
        }
    }
}

#[derive(Debug)]
/// Result returned for a logs subscription request.
pub struct ServiceLogSubscription {
    pub service_name: String,
    pub snapshot: ServiceLogSnapshot,
    pub receiver: Option<broadcast::Receiver<ServiceLogEvent>>,
}

#[derive(Debug)]
/// Internal mutable state for one supervised runner process.
struct RunningService {
    release_hash: String,
    started_at: String,
    status: RunningStatus,
    is_ready: bool,
    runner_id: String,
    runner_endpoint: PathBuf,
    manager_auth_secret: String,
    invocation_secret: String,
    bindings: Vec<ServiceBinding>,
    child: Child,
    composite_log: Arc<Mutex<CompositeLogBuffer>>,
    log_sender: broadcast::Sender<ServiceLogEvent>,
    last_heartbeat_at: String,
}

#[derive(Debug, Clone)]
struct StartFailureLogBuffers {
    composite_log: Arc<Mutex<CompositeLogBuffer>>,
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
    http_worker_count: u32,
    http_worker_queue_capacity: u32,
    http_queue_memory_budget_bytes: u64,
    wasm_memory_reservation_bytes: u64,
    wasm_memory_reservation_for_growth_bytes: u64,
    wasm_memory_guard_size_bytes: u64,
    wasm_guard_before_linear_memory: bool,
    wasm_parallel_compilation: bool,
    runner_log_buffer_bytes: usize,
    epoch_tick_interval_ms: u64,
    manager_control_endpoint: PathBuf,
    inner: Arc<RwLock<BTreeMap<String, RunningService>>>,
    runner_index: Arc<RwLock<RunnerServiceIndex>>,
    retained_logs: Arc<Mutex<RetainedFileLogStore>>,
    stopping_services: Arc<StdMutex<StoppingServicesMap>>,
    pending_ready: Arc<Mutex<PendingReadyMap>>,
    starting_services: Arc<Mutex<BTreeSet<String>>>,
    stopping_count: Arc<AtomicUsize>,
    _manager_control_handler: Arc<DefaultManagerControlHandler>,
    runner_spawner: DefaultRunnerSpawner,
    _manager_control_server: Arc<ManagerControlServer>,
}

impl ServiceSupervisor {
    /// Creates a service supervisor and starts manager control socket server.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage_root: impl AsRef<Path>,
        stop_grace_timeout_secs: u64,
        runner_ready_timeout_secs: u64,
        manager_control_read_timeout_ms: u64,
        http_worker_count: u32,
        http_worker_queue_capacity: u32,
        runner_log_buffer_bytes: usize,
        epoch_tick_interval_ms: u64,
    ) -> Result<Self, ImagodError> {
        let storage_root = storage_root.as_ref().to_path_buf();
        let default_config_path = storage_root.join("imagod.toml");
        Self::new_with_config_path(
            storage_root,
            stop_grace_timeout_secs,
            runner_ready_timeout_secs,
            manager_control_read_timeout_ms,
            http_worker_count,
            http_worker_queue_capacity,
            runner_log_buffer_bytes,
            runner_log_buffer_bytes.saturating_mul(2),
            epoch_tick_interval_ms,
            default_config_path,
        )
    }

    /// Creates a service supervisor and starts manager control socket server
    /// with an explicit `imagod.toml` path for manager-side remote RPC TOFU.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_config_path(
        storage_root: impl AsRef<Path>,
        stop_grace_timeout_secs: u64,
        runner_ready_timeout_secs: u64,
        manager_control_read_timeout_ms: u64,
        http_worker_count: u32,
        http_worker_queue_capacity: u32,
        runner_log_buffer_bytes: usize,
        retained_logs_capacity_bytes: usize,
        epoch_tick_interval_ms: u64,
        config_path: impl AsRef<Path>,
    ) -> Result<Self, ImagodError> {
        let config_path = config_path.as_ref().to_path_buf();
        let storage_root = storage_root.as_ref().to_path_buf();
        let runtime_root = storage_root.join("runtime").join("ipc");
        let stop_grace_timeout = Duration::from_secs(stop_grace_timeout_secs.max(1));
        let runner_ready_timeout = Duration::from_secs(runner_ready_timeout_secs.max(1));
        let manager_control_read_timeout =
            Duration::from_millis(manager_control_read_timeout_ms.max(1));
        let runner_log_buffer_bytes = runner_log_buffer_bytes.max(1024);
        let retained_logs_capacity_bytes = retained_logs_capacity_bytes.max(1);

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
        validate_unix_socket_path_len(&manager_control_endpoint, "manager control endpoint")?;
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
        let runner_index = Arc::new(RwLock::new(BTreeMap::new()));
        let retained_logs = Arc::new(Mutex::new(RetainedFileLogStore::new(
            &storage_root,
            retained_logs_capacity_bytes,
        )?));
        let stopping_services = Arc::new(StdMutex::new(BTreeMap::new()));
        let pending_ready = Arc::new(Mutex::new(BTreeMap::new()));
        let starting_services = Arc::new(Mutex::new(BTreeSet::new()));
        let stopping_count = Arc::new(AtomicUsize::new(0));
        let manager_control_handler = Arc::new(
            DefaultManagerControlHandler::new_with_runner_index(config_path, runner_index.clone()),
        );
        let runner_spawner = DefaultRunnerSpawner;
        let manager_control_server = Arc::new(Self::spawn_manager_control_server(
            listener,
            manager_control_endpoint.clone(),
            inner.clone(),
            pending_ready.clone(),
            manager_control_read_timeout,
            manager_control_handler.clone(),
        ));

        let supervisor = Self {
            storage_root,
            stop_grace_timeout,
            runner_ready_timeout,
            http_worker_count,
            http_worker_queue_capacity,
            http_queue_memory_budget_bytes: DEFAULT_HTTP_QUEUE_MEMORY_BUDGET_BYTES,
            wasm_memory_reservation_bytes: DEFAULT_WASM_MEMORY_RESERVATION_BYTES,
            wasm_memory_reservation_for_growth_bytes:
                DEFAULT_WASM_MEMORY_RESERVATION_FOR_GROWTH_BYTES,
            wasm_memory_guard_size_bytes: DEFAULT_WASM_MEMORY_GUARD_SIZE_BYTES,
            wasm_guard_before_linear_memory: DEFAULT_WASM_GUARD_BEFORE_LINEAR_MEMORY,
            wasm_parallel_compilation: DEFAULT_WASM_PARALLEL_COMPILATION,
            runner_log_buffer_bytes,
            epoch_tick_interval_ms: epoch_tick_interval_ms.max(1),
            manager_control_endpoint,
            inner,
            runner_index,
            retained_logs,
            stopping_services,
            pending_ready,
            starting_services,
            stopping_count,
            _manager_control_handler: manager_control_handler,
            runner_spawner,
            _manager_control_server: manager_control_server,
        };
        Ok(supervisor)
    }

    /// Overrides queued HTTP request memory budget in bytes.
    pub fn with_http_queue_memory_budget_bytes(
        mut self,
        http_queue_memory_budget_bytes: u64,
    ) -> Self {
        self.http_queue_memory_budget_bytes = http_queue_memory_budget_bytes;
        self
    }

    /// Overrides Wasmtime linear-memory reservation and guard tuning for runners.
    pub fn with_wasm_engine_tuning(
        mut self,
        wasm_memory_reservation_bytes: u64,
        wasm_memory_reservation_for_growth_bytes: u64,
        wasm_memory_guard_size_bytes: u64,
        wasm_guard_before_linear_memory: bool,
        wasm_parallel_compilation: bool,
    ) -> Self {
        self.wasm_memory_reservation_bytes = wasm_memory_reservation_bytes;
        self.wasm_memory_reservation_for_growth_bytes = wasm_memory_reservation_for_growth_bytes;
        self.wasm_memory_guard_size_bytes = wasm_memory_guard_size_bytes;
        self.wasm_guard_before_linear_memory = wasm_guard_before_linear_memory;
        self.wasm_parallel_compilation = wasm_parallel_compilation;
        self
    }

    /// Starts a service by spawning a runner child process.
    pub async fn start(&self, launch: ServiceLaunch) -> Result<(), ImagodError> {
        self.start_internal(launch, false).await
    }

    async fn start_internal(
        &self,
        launch: ServiceLaunch,
        include_wasm_log_details_on_failure: bool,
    ) -> Result<(), ImagodError> {
        self.reap_finished_service(&launch.name).await;
        self.reserve_start(&launch.name).await?;
        let service_name = launch.name.clone();
        let result = async {
            let effective_http_worker_queue_capacity =
                self.effective_http_worker_queue_capacity(&launch)?;
            let runner_id = uuid::Uuid::new_v4().to_string();
            let manager_auth_secret = random_secret_hex();
            let invocation_secret = random_secret_hex();
            let runner_endpoint = self.runner_endpoint_for(&launch.name, &runner_id);

            let bootstrap = RunnerBootstrap {
                runner_id: runner_id.clone(),
                service_name: launch.name.clone(),
                release_hash: launch.release_hash.clone(),
                app_type: launch.app_type,
                http_port: launch.http_port,
                http_max_body_bytes: launch.http_max_body_bytes,
                http_worker_count: self.http_worker_count,
                http_worker_queue_capacity: effective_http_worker_queue_capacity,
                socket: launch.socket.clone(),
                component_path: launch.component_path.clone(),
                args: launch.args.clone(),
                envs: launch.envs.clone(),
                wasi_mounts: launch.wasi_mounts.clone(),
                wasi_http_outbound: launch.wasi_http_outbound.clone(),
                resources: launch.resources.clone(),
                bindings: launch.bindings.clone(),
                plugin_dependencies: launch.plugin_dependencies.clone(),
                capabilities: launch.capabilities.clone(),
                manager_control_endpoint: self.manager_control_endpoint.clone(),
                runner_endpoint: runner_endpoint.clone(),
                manager_auth_secret: manager_auth_secret.clone(),
                invocation_secret: invocation_secret.clone(),
                epoch_tick_interval_ms: self.epoch_tick_interval_ms,
                wasm_memory_reservation_bytes: self.wasm_memory_reservation_bytes,
                wasm_memory_reservation_for_growth_bytes: self
                    .wasm_memory_reservation_for_growth_bytes,
                wasm_memory_guard_size_bytes: self.wasm_memory_guard_size_bytes,
                wasm_guard_before_linear_memory: self.wasm_guard_before_linear_memory,
                wasm_parallel_compilation: self.wasm_parallel_compilation,
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

            let composite_log = Arc::new(Mutex::new(CompositeLogBuffer::new(
                self.runner_log_buffer_bytes,
            )));
            let start_failure_log_buffers = StartFailureLogBuffers {
                composite_log: composite_log.clone(),
            };
            let (log_sender, _) = broadcast::channel(LOG_CHANNEL_CAPACITY);

            if let Some(stdout) = child.stdout.take() {
                spawn_log_drain(
                    stdout,
                    composite_log.clone(),
                    log_sender.clone(),
                    launch.name.clone(),
                    "stdout",
                    ServiceLogStream::Stdout,
                );
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_log_drain(
                    stderr,
                    composite_log.clone(),
                    log_sender.clone(),
                    launch.name.clone(),
                    "stderr",
                    ServiceLogStream::Stderr,
                );
            }

            {
                let mut inner = self.inner.write().await;
                inner.insert(
                    launch.name.clone(),
                    RunningService {
                        release_hash: launch.release_hash,
                        started_at: now_unix_secs().to_string(),
                        status: RunningStatus::Running,
                        is_ready: false,
                        runner_id: runner_id.clone(),
                        runner_endpoint,
                        manager_auth_secret,
                        invocation_secret,
                        bindings: launch.bindings,
                        child,
                        composite_log,
                        log_sender,
                        last_heartbeat_at: now_unix_secs().to_string(),
                    },
                );
            }
            self.upsert_runner_index(&runner_id, &launch.name).await;

            if let Err(err) = self
                .write_bootstrap_to_running_service(&launch.name, &bootstrap)
                .await
            {
                let err = attach_start_failure_wasm_log_details(
                    err,
                    include_wasm_log_details_on_failure,
                    &start_failure_log_buffers,
                )
                .await;
                self.pending_ready.lock().await.remove(&runner_id);
                self.cleanup_start_failure(&launch.name).await;
                return Err(err);
            }

            let ready_result = self
                .wait_for_runner_ready(&launch.name, &runner_id, &mut ready_rx)
                .await;
            self.pending_ready.lock().await.remove(&runner_id);

            if let Err(err) = ready_result {
                let err = attach_start_failure_wasm_log_details(
                    err,
                    include_wasm_log_details_on_failure,
                    &start_failure_log_buffers,
                )
                .await;
                self.cleanup_start_failure(&launch.name).await;
                return Err(err);
            }

            Ok(())
        }
        .await;
        self.release_start(&service_name).await;
        result
    }

    fn effective_http_worker_queue_capacity(
        &self,
        launch: &ServiceLaunch,
    ) -> Result<u32, ImagodError> {
        if launch.app_type != RunnerAppType::Http {
            return Ok(self.http_worker_queue_capacity);
        }

        let Some(max_body_bytes) = launch.http_max_body_bytes else {
            return Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!(
                    "service '{}' is missing manifest.http.max_body_bytes for app_type=http",
                    launch.name
                ),
            ));
        };

        if max_body_bytes == 0 {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_START,
                format!(
                    "service '{}' must declare manifest.http.max_body_bytes greater than zero",
                    launch.name
                ),
            ));
        }

        let max_queue_by_budget = self.http_queue_memory_budget_bytes / max_body_bytes;
        if max_queue_by_budget == 0 {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_START,
                format!(
                    "service '{}' exceeds runtime.http_queue_memory_budget_bytes: budget={} max_body_bytes={}",
                    launch.name, self.http_queue_memory_budget_bytes, max_body_bytes
                ),
            ));
        }

        let configured_capacity = u64::from(self.http_worker_queue_capacity);
        let effective_capacity = configured_capacity.min(max_queue_by_budget);
        u32::try_from(effective_capacity).map_err(|_| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!("effective HTTP queue capacity is too large: {effective_capacity}"),
            )
        })
    }

    /// Replaces an existing service using stop-then-start sequence.
    pub async fn replace(&self, launch: ServiceLaunch) -> Result<(), ImagodError> {
        match self.stop(&launch.name, false).await {
            Ok(()) => {}
            Err(err) if err.code == ErrorCode::NotFound => {}
            Err(err) => return Err(err),
        }
        self.start_internal(launch, true).await
    }

    /// Stops a running service, optionally forcing immediate kill.
    pub async fn stop(&self, service_name: &str, force: bool) -> Result<(), ImagodError> {
        let _stopping_service_guard = self.begin_stop_service(service_name);
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
            self.remove_runner_index(&service.runner_id).await;
            remove_runner_endpoint_best_effort(&service.runner_endpoint);
            self.retain_composite_snapshot(service_name, &service.composite_log)
                .await;
            return Err(ImagodError::new(
                ErrorCode::NotFound,
                STAGE_STOP,
                format!("service '{service_name}' is not running"),
            ));
        }

        service.status = RunningStatus::Stopping;

        let stop_result = async {
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
        .await;

        match stop_result {
            Ok(()) => {
                remove_runner_endpoint_best_effort(&service.runner_endpoint);
                self.retain_composite_snapshot(service_name, &service.composite_log)
                    .await;
                Ok(())
            }
            Err(err) => {
                self.restore_service_after_stop_error(service_name, service)
                    .await;
                Err(err)
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
            self.remove_runner_index(&service.runner_id).await;
            remove_runner_endpoint_best_effort(&service.runner_endpoint);
            self.retain_composite_snapshot(&name, &service.composite_log)
                .await;
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

    /// Returns currently running service names.
    pub async fn running_service_names(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner
            .iter()
            .filter_map(|(name, service)| {
                if service.status == RunningStatus::Running {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns runtime state snapshots for all tracked services.
    pub async fn runtime_service_states(&self) -> Vec<RuntimeServiceState> {
        let inner = self.inner.read().await;
        inner
            .iter()
            .map(|(name, service)| RuntimeServiceState {
                name: name.clone(),
                release_hash: service.release_hash.clone(),
                started_at: service.started_at.clone(),
                status: service.status,
            })
            .collect()
    }

    /// Returns service names that can provide log snapshots (running + retained).
    pub async fn loggable_service_names(&self) -> Vec<String> {
        let running_names = self.running_service_names().await;
        let retained_names = {
            let retained = self.retained_logs.lock().await;
            retained.service_names()
        };
        let stopping_names = self.stopping_service_names();

        let mut merged = BTreeSet::new();
        merged.extend(running_names);
        merged.extend(retained_names);
        merged.retain(|name| !stopping_names.contains(name));
        merged.into_iter().collect()
    }

    /// Opens one service log snapshot and optional follow stream.
    pub async fn open_logs(
        &self,
        service_name: &str,
        tail_lines: u32,
        follow: bool,
        with_timestamp: bool,
    ) -> Result<ServiceLogSubscription, ImagodError> {
        let running_subscription = {
            let inner = self.inner.read().await;
            match inner.get(service_name) {
                Some(service) if service.status == RunningStatus::Running => {
                    let receiver = if follow {
                        Some(service.log_sender.subscribe())
                    } else {
                        None
                    };
                    Some((service.composite_log.clone(), receiver))
                }
                _ => None,
            }
        };

        if let Some((snapshot_source, receiver)) = running_subscription {
            let snapshot = {
                let buffer = snapshot_source.lock().await;
                if with_timestamp {
                    ServiceLogSnapshot::Events(buffer.tail_snapshot_events(tail_lines))
                } else {
                    ServiceLogSnapshot::Bytes(buffer.tail_snapshot_bytes(tail_lines))
                }
            };

            return Ok(ServiceLogSubscription {
                service_name: service_name.to_string(),
                snapshot,
                receiver,
            });
        }

        if self.is_service_stopping(service_name) {
            return Err(ImagodError::new(
                ErrorCode::NotFound,
                STAGE_LOGS,
                format!("service '{service_name}' is not running"),
            ));
        }

        let retained_events = self.read_retained_events(service_name).await?;
        let retained_events = retained_events.ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                STAGE_LOGS,
                format!("service '{service_name}' has no available logs"),
            )
        })?;

        let snapshot = if with_timestamp {
            ServiceLogSnapshot::Events(tail_snapshot_events_from_events(
                &retained_events,
                tail_lines,
            ))
        } else {
            ServiceLogSnapshot::Bytes(tail_snapshot_bytes_from_events(
                &retained_events,
                tail_lines,
            ))
        };

        Ok(ServiceLogSubscription {
            service_name: service_name.to_string(),
            snapshot,
            receiver: None,
        })
    }

    /// Invokes one interface function on a running target service.
    pub async fn invoke(
        &self,
        target_service_name: &str,
        interface_id: &str,
        function: &str,
        payload_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, ImagodError> {
        let (runner_endpoint, invocation_secret) = {
            let inner = self.inner.read().await;
            let target_service = inner.get(target_service_name).ok_or_else(|| {
                ImagodError::new(
                    ErrorCode::NotFound,
                    STAGE_INVOKE,
                    format!("service '{target_service_name}' is not running"),
                )
            })?;
            if target_service.status != RunningStatus::Running || !target_service.is_ready {
                return Err(ImagodError::new(
                    ErrorCode::NotFound,
                    STAGE_INVOKE,
                    format!("service '{target_service_name}' is not running"),
                ));
            }
            (
                target_service.runner_endpoint.clone(),
                target_service.invocation_secret.clone(),
            )
        };

        let claims = InvocationTokenClaims {
            source_service: "remote".to_string(),
            target_service: target_service_name.to_string(),
            wit: interface_id.to_string(),
            exp: now_unix_secs() + INVOCATION_TOKEN_TTL_SECS,
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let token = issue_invocation_token(&invocation_secret, claims)?;

        let response = DbusP2pTransport::call_runner(
            &runner_endpoint,
            &RunnerInboundRequest::Invoke {
                interface_id: interface_id.to_string(),
                function: function.to_string(),
                payload_cbor,
                token,
            },
        )
        .await?;

        match response {
            RunnerInboundResponse::InvokeResult { payload_cbor } => Ok(payload_cbor),
            RunnerInboundResponse::Error(err) => Err(err.into()),
            RunnerInboundResponse::Ack => Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_INVOKE,
                "unexpected ack response for invoke",
            )),
        }
    }

    /// Spawns the async manager control server loop on the provided listener.
    fn spawn_manager_control_server(
        listener: UnixListener,
        endpoint: PathBuf,
        inner: Arc<RwLock<BTreeMap<String, RunningService>>>,
        pending_ready: Arc<Mutex<PendingReadyMap>>,
        manager_control_read_timeout: Duration,
        manager_control_handler: Arc<DefaultManagerControlHandler>,
    ) -> ManagerControlServer {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let concurrency = Arc::new(Semaphore::new(MAX_MANAGER_CONTROL_CONNECTION_HANDLERS));
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                        continue;
                    }
                };

                let (stream, _) = match accepted {
                    Ok(v) => v,
                    Err(err) => {
                        eprintln!("manager control accept failed: {err}");
                        continue;
                    }
                };

                let permit = tokio::select! {
                    acquired = concurrency.clone().acquire_owned() => {
                        match acquired {
                            Ok(permit) => permit,
                            Err(_) => break,
                        }
                    }
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                        continue;
                    }
                };

                let inner = inner.clone();
                let pending_ready = pending_ready.clone();
                let manager_control_handler = manager_control_handler.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    manager_control_handler
                        .handle_control_connection(
                            stream,
                            inner,
                            pending_ready,
                            manager_control_read_timeout,
                        )
                        .await;
                });
            }
        });

        ManagerControlServer {
            endpoint,
            shutdown_tx,
            task,
        }
    }

    /// Spawns the `imagod --runner` child process with piped stdio.
    fn spawn_runner_child(&self, bootstrap: &RunnerBootstrap) -> Result<Child, ImagodError> {
        self.runner_spawner.spawn_runner_child(bootstrap)
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
        let mut ready_timeout = std::pin::pin!(time::sleep(self.runner_ready_timeout));
        let mut runner_exit_wait = std::pin::pin!(self.wait_for_runner_exit(service_name));
        tokio::select! {
            ready = &mut *ready_rx => match ready {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(err),
                Err(_) => Err(self.runner_ready_channel_closed_error(runner_id)),
            },
            _ = &mut runner_exit_wait => Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_START,
                format!("service '{service_name}' exited before ready"),
            )),
            _ = &mut ready_timeout => {
                match ready_rx.try_recv() {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(err)) => Err(err),
                    Err(TryRecvError::Closed) => Err(self.runner_ready_channel_closed_error(runner_id)),
                    Err(TryRecvError::Empty) => Err(ImagodError::new(
                        ErrorCode::OperationTimeout,
                        STAGE_START,
                        format!("service '{service_name}' did not send runner_ready in time"),
                    )),
                }
            }
        }
    }

    async fn wait_for_runner_exit(&self, service_name: &str) {
        let mut interval = time::interval(Duration::from_millis(STARTUP_EXIT_CHECK_INTERVAL_MS));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let exited = {
                let mut inner = self.inner.write().await;
                match inner.get_mut(service_name) {
                    Some(service) => matches!(service.child.try_wait(), Ok(Some(_))),
                    None => true,
                }
            };
            if exited {
                return;
            }
        }
    }

    fn runner_ready_channel_closed_error(&self, runner_id: &str) -> ImagodError {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_START,
            format!("runner '{runner_id}' readiness channel closed unexpectedly"),
        )
    }

    fn begin_stop_service(&self, service_name: &str) -> StoppingServiceGuard {
        {
            let mut stopping_services = match self.stopping_services.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let entry = stopping_services
                .entry(service_name.to_string())
                .or_insert(0);
            *entry = entry.saturating_add(1);
        }
        StoppingServiceGuard {
            service_name: service_name.to_string(),
            stopping_services: self.stopping_services.clone(),
        }
    }

    fn stopping_service_names(&self) -> BTreeSet<String> {
        let stopping_services = match self.stopping_services.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        stopping_services.keys().cloned().collect()
    }

    fn is_service_stopping(&self, service_name: &str) -> bool {
        let stopping_services = match self.stopping_services.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        stopping_services.contains_key(service_name)
    }

    async fn reserve_start(&self, service_name: &str) -> Result<(), ImagodError> {
        {
            let mut starting_services = self.starting_services.lock().await;
            if starting_services.contains(service_name) {
                return Err(start_busy_error(service_name));
            }
            starting_services.insert(service_name.to_string());
        }

        // Keep exited-runner cleanup logic centralized to avoid divergence.
        self.reap_finished_service(service_name).await;

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

    async fn upsert_runner_index(&self, runner_id: &str, service_name: &str) {
        self.runner_index
            .write()
            .await
            .insert(runner_id.to_string(), service_name.to_string());
    }

    async fn remove_runner_index(&self, runner_id: &str) {
        self.runner_index.write().await.remove(runner_id);
    }

    async fn cleanup_start_failure(&self, service_name: &str) {
        let service = {
            let mut inner = self.inner.write().await;
            inner.remove(service_name)
        };
        if let Some(mut service) = service {
            self.remove_runner_index(&service.runner_id).await;
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
        let finished = {
            let mut inner = self.inner.write().await;
            match inner.get_mut(service_name) {
                Some(service) => match service.child.try_wait() {
                    Ok(Some(status)) => inner
                        .remove(service_name)
                        .map(|running_service| (running_service, status)),
                    Ok(None) => None,
                    Err(err) => {
                        eprintln!(
                            "service try_wait failed name={} release={} error={}",
                            service_name, service.release_hash, err
                        );
                        None
                    }
                },
                None => None,
            }
        };

        if let Some((service, exit_status)) = finished {
            self.remove_runner_index(&service.runner_id).await;
            log_exit_outcome(
                service_name,
                &service.release_hash,
                &service.started_at,
                service.status,
                exit_status,
            );
            remove_runner_endpoint_best_effort(&service.runner_endpoint);
            self.retain_composite_snapshot(service_name, &service.composite_log)
                .await;
            self.pending_ready
                .lock()
                .await
                .retain(|_, sender| !sender.is_closed());
        }
    }

    async fn retain_composite_snapshot(
        &self,
        service_name: &str,
        composite_log: &Arc<Mutex<CompositeLogBuffer>>,
    ) {
        let snapshot_events = {
            let buffer = composite_log.lock().await;
            buffer.snapshot_events()
        };

        if let Err(err) = self
            .write_retained_snapshot(service_name.to_string(), snapshot_events)
            .await
        {
            eprintln!(
                "service retained log snapshot write failed name={} error={}",
                service_name, err
            );
        }
    }

    async fn read_retained_events(
        &self,
        service_name: &str,
    ) -> Result<Option<Vec<ServiceLogEvent>>, ImagodError> {
        let retained_logs = self.retained_logs.clone();
        let service_name = service_name.to_string();
        tokio::task::spawn_blocking(move || {
            let retained = retained_logs.blocking_lock();
            retained.snapshot_events(&service_name)
        })
        .await
        .map_err(|err| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_LOGS,
                format!("retained log snapshot task failed: {err}"),
            )
        })?
    }

    async fn write_retained_snapshot(
        &self,
        service_name: String,
        snapshot_events: Vec<ServiceLogEvent>,
    ) -> Result<(), ImagodError> {
        let retained_logs = self.retained_logs.clone();
        tokio::task::spawn_blocking(move || {
            let mut retained = retained_logs.blocking_lock();
            retained.upsert(&service_name, &snapshot_events)
        })
        .await
        .map_err(|err| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_LOGS,
                format!("retained log write task failed: {err}"),
            )
        })?
    }

    async fn take_running(&self, service_name: &str) -> Result<RunningService, ImagodError> {
        let service = {
            let mut inner = self.inner.write().await;
            inner.remove(service_name)
        };
        let Some(service) = service else {
            return Err(ImagodError::new(
                ErrorCode::NotFound,
                STAGE_STOP,
                format!("service '{service_name}' is not running"),
            ));
        };
        self.remove_runner_index(&service.runner_id).await;
        Ok(service)
    }

    async fn restore_service_after_stop_error(
        &self,
        service_name: &str,
        mut service: RunningService,
    ) {
        match service.child.try_wait() {
            Ok(Some(exit_status)) => {
                log_exit_outcome(
                    service_name,
                    &service.release_hash,
                    &service.started_at,
                    service.status,
                    exit_status,
                );
                remove_runner_endpoint_best_effort(&service.runner_endpoint);
                return;
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!(
                    "service stop recovery try_wait failed name={} release={} error={}",
                    service_name, service.release_hash, err
                );
            }
        }

        service.status = RunningStatus::Running;
        let mut inner = self.inner.write().await;
        if inner.contains_key(service_name) {
            eprintln!(
                "service stop recovery skipped insert because '{}' already exists",
                service_name
            );
            drop(inner);
            match kill_and_wait(&mut service.child).await {
                Ok(()) => {
                    remove_runner_endpoint_best_effort(&service.runner_endpoint);
                }
                Err(err) => {
                    eprintln!(
                        "service stop recovery failed to terminate displaced runner name={} release={} error={}",
                        service_name, service.release_hash, err
                    );
                }
            }
            return;
        }
        let runner_id = service.runner_id.clone();
        inner.insert(service_name.to_string(), service);
        drop(inner);
        self.upsert_runner_index(&runner_id, service_name).await;
    }
}

async fn attach_start_failure_wasm_log_details(
    mut err: ImagodError,
    include_wasm_log_details_on_failure: bool,
    log_buffers: &StartFailureLogBuffers,
) -> ImagodError {
    if !include_wasm_log_details_on_failure {
        return err;
    }

    let snapshot_events = {
        let buffer = log_buffers.composite_log.lock().await;
        buffer.snapshot_events()
    };

    let stdout = collect_stream_snapshot_bytes(&snapshot_events, ServiceLogStream::Stdout);
    if !stdout.is_empty() {
        let stdout_text = String::from_utf8_lossy(&stdout).into_owned();
        if !stdout_text.is_empty() {
            err = err.with_detail(DETAIL_WASM_STDOUT, stdout_text);
        }
    }

    let stderr = collect_stream_snapshot_bytes(&snapshot_events, ServiceLogStream::Stderr);
    if !stderr.is_empty() {
        let stderr_text = String::from_utf8_lossy(&stderr).into_owned();
        if !stderr_text.is_empty() {
            err = err.with_detail(DETAIL_WASM_STDERR, stderr_text);
        }
    }

    err
}

fn collect_stream_snapshot_bytes(events: &[ServiceLogEvent], stream: ServiceLogStream) -> Vec<u8> {
    let mut out = Vec::new();
    for event in events {
        if event.stream == stream {
            out.extend_from_slice(&event.bytes);
        }
    }
    out
}

fn build_runner_endpoint(storage_root: &Path, service_name: &str, runner_id: &str) -> PathBuf {
    runner_spawn::build_runner_endpoint(storage_root, service_name, runner_id)
}

fn validate_unix_socket_path_len(path: &Path, socket_name: &str) -> Result<(), ImagodError> {
    runner_spawn::validate_unix_socket_path_len(path, socket_name)
}

/// Handles one control request received on manager control socket.
#[cfg(test)]
async fn handle_control_request(
    inner: &Arc<RwLock<BTreeMap<String, RunningService>>>,
    pending_ready: &Arc<Mutex<PendingReadyMap>>,
    request: ControlRequest,
) -> ControlResponse {
    let handler =
        manager_control::DefaultManagerControlHandler::new(PathBuf::from("/tmp/imagod.toml"));
    manager_control::handle_control_request_impl(inner, pending_ready, &handler, request).await
}

/// Validates manager proof generated from shared secret and runner id.
#[cfg(test)]
fn validate_manager_auth(secret: &str, runner_id: &str, proof: &str) -> Result<(), ImagodError> {
    manager_control::validate_manager_auth(secret, runner_id, proof)
}

/// Returns whether a binding list allows the target service/interface pair.
#[cfg(test)]
fn is_binding_allowed(bindings: &[ServiceBinding], target_service: &str, wit: &str) -> bool {
    manager_control::is_binding_allowed(bindings, target_service, wit)
}

/// Drains one child output stream into bounded in-memory log buffer.
///
/// Concurrency: runs as a detached task per stream.
#[allow(clippy::too_many_arguments)]
fn spawn_log_drain<R>(
    reader: R,
    composite_buffer: Arc<Mutex<CompositeLogBuffer>>,
    sender: broadcast::Sender<ServiceLogEvent>,
    service_name: String,
    stream_name: &'static str,
    stream: ServiceLogStream,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    log_buffer::spawn_log_drain(
        reader,
        composite_buffer,
        sender,
        service_name,
        stream_name,
        stream,
    );
}

#[cfg(test)]
fn snapshot_bytes_from_events(events: &[ServiceLogEvent]) -> Vec<u8> {
    log_buffer::snapshot_bytes_from_events(events)
}

fn tail_snapshot_bytes_from_events(events: &[ServiceLogEvent], tail_lines: u32) -> Vec<u8> {
    log_buffer::tail_snapshot_bytes_from_events(events, tail_lines)
}

fn tail_snapshot_events_from_events(
    events: &[ServiceLogEvent],
    tail_lines: u32,
) -> Vec<ServiceLogEvent> {
    log_buffer::tail_snapshot_events_from_events(events, tail_lines)
}

/// Sends kill signal to child and waits for termination.
#[allow(clippy::collapsible_if)]
async fn kill_and_wait(child: &mut Child) -> Result<(), ImagodError> {
    #[cfg(test)]
    {
        if let Some(pid) = child.id() {
            if FAIL_KILL_AND_WAIT_FOR_PID
                .compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Err(ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_STOP,
                    format!("injected kill_and_wait failure for pid {pid}"),
                ));
            }
        }
    }

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

struct StoppingServiceGuard {
    service_name: String,
    stopping_services: Arc<StdMutex<StoppingServicesMap>>,
}

impl Drop for StoppingServiceGuard {
    fn drop(&mut self) {
        let mut stopping_services = match self.stopping_services.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(count) = stopping_services.get_mut(&self.service_name) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                stopping_services.remove(&self.service_name);
            }
        }
    }
}

#[cfg(test)]
static FAIL_KILL_AND_WAIT_FOR_PID: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

#[cfg(test)]
fn inject_kill_and_wait_failure_for_pid(pid: u32) {
    FAIL_KILL_AND_WAIT_FOR_PID.store(pid, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_store::ArtifactStore;
    use imagod_spec_formal::{
        RuntimeProjectionAction, RuntimeProjectionObservedState, RuntimeProjectionSpec,
        SystemEffect, SystemState, atoms::ServiceAtom,
    };
    use nirvash_core::{
        ProtocolConformanceSpec, TransitionSystem,
        conformance::{
            ActionApplier as ProtocolActionApplier, ProtocolRuntimeBinding, StateObserver,
        },
    };
    use nirvash_macros::code_tests;
    use sha2::{Digest, Sha256};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tar::{Builder, Header};
    use tokio::{net::UnixListener, process::Command};

    const STAGE_RUNTIME_PROJECTION: &str = "runtime_projection";

    fn new_test_root(prefix: &str) -> PathBuf {
        let _ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
        PathBuf::from(format!("/tmp/iss-{prefix}-{id}"))
    }

    fn expect_snapshot_bytes(snapshot: &ServiceLogSnapshot) -> &[u8] {
        snapshot
            .as_bytes()
            .expect("snapshot should be bytes-based for this assertion")
    }

    fn expect_snapshot_events(snapshot: &ServiceLogSnapshot) -> &[ServiceLogEvent] {
        snapshot
            .as_events()
            .expect("snapshot should be event-based for this assertion")
    }

    fn new_running_service(
        child: Child,
        runner_id: &str,
        runner_endpoint: PathBuf,
    ) -> RunningService {
        let (log_sender, _) = broadcast::channel(LOG_CHANNEL_CAPACITY);
        RunningService {
            release_hash: "release-a".to_string(),
            started_at: now_unix_secs().to_string(),
            status: RunningStatus::Running,
            is_ready: true,
            runner_id: runner_id.to_string(),
            runner_endpoint,
            manager_auth_secret: random_secret_hex(),
            invocation_secret: random_secret_hex(),
            bindings: Vec::new(),
            child,
            composite_log: Arc::new(Mutex::new(CompositeLogBuffer::new(128))),
            log_sender,
            last_heartbeat_at: now_unix_secs().to_string(),
        }
    }

    async fn stop_running_service_best_effort(
        inner: &Arc<RwLock<BTreeMap<String, RunningService>>>,
        service_name: &str,
    ) {
        let service = {
            let mut guard = inner.write().await;
            guard.remove(service_name)
        };
        if let Some(mut service) = service {
            let _ = kill_and_wait(&mut service.child).await;
        }
    }

    fn sample_http_launch(max_body_bytes: u64) -> ServiceLaunch {
        ServiceLaunch {
            name: "svc-http".to_string(),
            release_hash: "release-test".to_string(),
            app_type: RunnerAppType::Http,
            http_port: Some(18080),
            http_max_body_bytes: Some(max_body_bytes),
            socket: None,
            component_path: PathBuf::from("/tmp/unused.wasm"),
            args: Vec::new(),
            envs: std::collections::BTreeMap::new(),
            wasi_mounts: Vec::new(),
            wasi_http_outbound: Vec::new(),
            resources: ResourceMap::default(),
            bindings: Vec::new(),
            plugin_dependencies: Vec::new(),
            capabilities: CapabilityPolicy::default(),
        }
    }

    fn append_tar_file(builder: &mut Builder<Vec<u8>>, name: &str, bytes: &[u8]) {
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, bytes)
            .expect("tar entry should be appended");
    }

    fn artifact_archive(service_name: &str) -> (Vec<u8>, String, String) {
        let manifest = serde_json::json!({
            "name": service_name,
            "main": "component.wasm",
            "type": "rpc",
            "assets": [],
            "bindings": [],
            "dependencies": [],
            "capabilities": {},
            "hash": {
                "algorithm": "sha256",
                "targets": ["wasm", "manifest", "assets"],
            }
        });
        let manifest_bytes =
            serde_json::to_vec(&manifest).expect("manifest should serialize to JSON");
        let component_bytes = wat::parse_str("(component)").expect("component should compile");
        let mut builder = Builder::new(Vec::new());
        append_tar_file(&mut builder, "manifest.json", &manifest_bytes);
        append_tar_file(&mut builder, "component.wasm", &component_bytes);
        builder.finish().expect("artifact tar should finish");
        let artifact_bytes = builder
            .into_inner()
            .expect("artifact bytes should be returned");
        let artifact_digest = hex::encode(Sha256::digest(&artifact_bytes));
        let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
        (artifact_bytes, artifact_digest, manifest_digest)
    }

    #[derive(Debug, Default, Clone, Copy)]
    struct RuntimeProjectionBinding;

    struct RuntimeProjectionRuntime {
        root: PathBuf,
        artifact_store: ArtifactStore,
        supervisor: ServiceSupervisor,
        state: tokio::sync::Mutex<SystemState>,
        trace: tokio::sync::Mutex<Vec<RuntimeProjectionAction>>,
    }

    impl RuntimeProjectionRuntime {
        async fn new() -> Self {
            let root = new_test_root("runtime-projection");
            let artifact_root = root.join("artifacts");
            std::fs::create_dir_all(&artifact_root).expect("artifact root should exist");
            let artifact_store =
                ArtifactStore::new(&artifact_root, 60, 60, 8, 64 * 1024, 4, 512 * 1024)
                    .await
                    .expect("artifact store should initialize");
            let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
                .expect("supervisor should initialize");
            Self {
                root,
                artifact_store,
                supervisor,
                state: tokio::sync::Mutex::new(RuntimeProjectionSpec::new().initial_state()),
                trace: tokio::sync::Mutex::new(Vec::new()),
            }
        }

        fn service_name(service: ServiceAtom) -> &'static str {
            match service {
                ServiceAtom::Service0 => "svc-source",
                ServiceAtom::Service1 => "svc-target",
            }
        }

        fn runner_id(service: ServiceAtom) -> &'static str {
            match service {
                ServiceAtom::Service0 => "runner-source",
                ServiceAtom::Service1 => "runner-target",
            }
        }

        fn release_hash(service: ServiceAtom) -> &'static str {
            match service {
                ServiceAtom::Service0 => "release-source-current",
                ServiceAtom::Service1 => "release-target-current",
            }
        }

        async fn ensure_release_markers(&self, service: ServiceAtom) -> Result<(), ImagodError> {
            let service_root = self.root.join("services").join(Self::service_name(service));
            tokio::fs::create_dir_all(service_root.join(Self::release_hash(service)))
                .await
                .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_START, e.to_string()))?;
            tokio::fs::create_dir_all(service_root.join("release-previous"))
                .await
                .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_START, e.to_string()))?;
            tokio::fs::write(
                service_root.join("active_release"),
                Self::release_hash(service).as_bytes(),
            )
            .await
            .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_START, e.to_string()))?;
            tokio::fs::write(service_root.join("restart_policy"), b"always")
                .await
                .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_START, e.to_string()))?;
            Ok(())
        }

        async fn upload_artifact(&self, service: ServiceAtom) -> Result<(), ImagodError> {
            let service_name = Self::service_name(service);
            let (artifact_bytes, artifact_digest, manifest_digest) = artifact_archive(service_name);
            let prepare = self
                .artifact_store
                .prepare(imagod_spec::DeployPrepareRequest {
                    name: service_name.to_string(),
                    app_type: "rpc".to_string(),
                    target: BTreeMap::new(),
                    artifact_digest: artifact_digest.clone(),
                    artifact_size: artifact_bytes.len() as u64,
                    manifest_digest: manifest_digest.clone(),
                    idempotency_key: format!("deploy-{service_name}"),
                    policy: BTreeMap::new(),
                })
                .await?;
            let chunk_sha256 = hex::encode(Sha256::digest(&artifact_bytes));
            self.artifact_store
                .push(imagod_spec::ArtifactPushRequest {
                    header: imagod_spec::ArtifactPushChunkHeader {
                        deploy_id: prepare.deploy_id.clone(),
                        offset: 0,
                        length: artifact_bytes.len() as u64,
                        chunk_sha256,
                        upload_token: prepare.upload_token.clone(),
                    },
                    chunk: artifact_bytes.clone(),
                })
                .await?;
            self.artifact_store
                .commit(imagod_spec::ArtifactCommitRequest {
                    deploy_id: prepare.deploy_id.clone(),
                    artifact_digest,
                    artifact_size: artifact_bytes.len() as u64,
                    manifest_digest,
                })
                .await?;
            let committed = self
                .artifact_store
                .committed_artifact(&prepare.deploy_id)
                .await?;
            assert_eq!(committed.deploy_id, prepare.deploy_id);
            self.ensure_release_markers(service).await?;
            Ok(())
        }

        async fn seed_running_service(&self, service: ServiceAtom) -> Result<(), ImagodError> {
            let child = Command::new("sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_START, e.to_string()))?;
            let mut running = new_running_service(
                child,
                Self::runner_id(service),
                self.root
                    .join("runtime")
                    .join("ipc")
                    .join(format!("{}.sock", Self::runner_id(service))),
            );
            running.release_hash = Self::release_hash(service).to_string();
            let mut guard = self.supervisor.inner.write().await;
            guard.insert(Self::service_name(service).to_string(), running);
            Ok(())
        }

        async fn deploy_service(&self, service: ServiceAtom) -> Result<(), ImagodError> {
            self.upload_artifact(service).await?;
            self.seed_running_service(service).await
        }

        async fn rollback_service0(&self) -> Result<(), ImagodError> {
            let service_root = self
                .root
                .join("services")
                .join(Self::service_name(ServiceAtom::Service0));
            tokio::fs::write(service_root.join("active_release"), b"release-previous")
                .await
                .map_err(|e| {
                    ImagodError::new(ErrorCode::Internal, STAGE_RUNTIME_PROJECTION, e.to_string())
                })?;
            Ok(())
        }

        async fn local_rpc_resolved(&self) -> Result<(), ImagodError> {
            let wit = "pkg:iface/invoke".to_string();
            let source_runner_id = Self::runner_id(ServiceAtom::Service0);
            {
                let mut guard = self.supervisor.inner.write().await;
                let source = guard
                    .get_mut(Self::service_name(ServiceAtom::Service0))
                    .expect("source service should be running");
                source.bindings = vec![ServiceBinding {
                    name: Self::service_name(ServiceAtom::Service1).to_string(),
                    wit: wit.clone(),
                }];
            }
            let source_secret = {
                let guard = self.supervisor.inner.read().await;
                guard
                    .get(Self::service_name(ServiceAtom::Service0))
                    .expect("source service should be present")
                    .manager_auth_secret
                    .clone()
            };
            let response = handle_control_request(
                &self.supervisor.inner,
                &self.supervisor.pending_ready,
                ControlRequest::ResolveInvocationTarget {
                    runner_id: source_runner_id.to_string(),
                    manager_auth_proof: compute_manager_auth_proof(
                        &source_secret,
                        source_runner_id,
                    )
                    .expect("proof should be generated"),
                    target_service: Self::service_name(ServiceAtom::Service1).to_string(),
                    wit,
                },
            )
            .await;
            match response {
                ControlResponse::ResolvedInvocationTarget { .. } => Ok(()),
                ControlResponse::Error(err) => {
                    Err(ImagodError::new(err.code, err.stage, err.message))
                }
                other => Err(ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_CONTROL,
                    format!("unexpected local rpc response: {other:?}"),
                )),
            }
        }

        async fn local_rpc_denied(&self) -> Result<(), ImagodError> {
            let source_runner_id = Self::runner_id(ServiceAtom::Service0);
            let Some(source_secret) = ({
                let guard = self.supervisor.inner.read().await;
                guard
                    .get(Self::service_name(ServiceAtom::Service0))
                    .map(|service| service.manager_auth_secret.clone())
            }) else {
                return Ok(());
            };
            match handle_control_request(
                &self.supervisor.inner,
                &self.supervisor.pending_ready,
                ControlRequest::ResolveInvocationTarget {
                    runner_id: source_runner_id.to_string(),
                    manager_auth_proof: compute_manager_auth_proof(
                        &source_secret,
                        source_runner_id,
                    )
                    .expect("proof should be generated"),
                    target_service: Self::service_name(ServiceAtom::Service1).to_string(),
                    wit: "pkg:iface/invoke".to_string(),
                },
            )
            .await
            {
                ControlResponse::Error(_) => Ok(()),
                other => Err(ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_CONTROL,
                    format!("expected rejected local rpc, got {other:?}"),
                )),
            }
        }

        async fn remote_rpc_lifecycle(&self) -> Result<(), ImagodError> {
            let (runner_endpoint, invocation_secret) = {
                let guard = self.supervisor.inner.read().await;
                let target = guard
                    .get(Self::service_name(ServiceAtom::Service0))
                    .expect("target service should be present");
                (
                    target.runner_endpoint.clone(),
                    target.invocation_secret.clone(),
                )
            };
            if let Some(parent) = runner_endpoint.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ImagodError::new(ErrorCode::Internal, STAGE_INVOKE, e.to_string())
                })?;
            }
            let _ = std::fs::remove_file(&runner_endpoint);
            let listener = UnixListener::bind(&runner_endpoint)
                .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_INVOKE, e.to_string()))?;
            let server_task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept should succeed");
                let request = DbusP2pTransport::read_message::<RunnerInboundRequest>(&mut stream)
                    .await
                    .expect("invoke request should decode");
                match request {
                    RunnerInboundRequest::Invoke {
                        interface_id,
                        function,
                        payload_cbor,
                        token,
                    } => {
                        assert_eq!(interface_id, "yieldspace:service/invoke");
                        assert_eq!(function, "call");
                        let claims =
                            imagod_ipc::verify_invocation_token(&invocation_secret, &token)
                                .expect("token should verify");
                        assert_eq!(claims.target_service, "svc-source");
                        DbusP2pTransport::write_message(
                            &mut stream,
                            &RunnerInboundResponse::InvokeResult { payload_cbor },
                        )
                        .await
                        .expect("invoke response should write");
                    }
                    other => panic!("unexpected request: {other:?}"),
                }
            });
            let payload = vec![0x01, 0x02, 0x03];
            let result = self
                .supervisor
                .invoke(
                    Self::service_name(ServiceAtom::Service0),
                    "yieldspace:service/invoke",
                    "call",
                    payload.clone(),
                )
                .await?;
            assert_eq!(result, payload);
            server_task
                .await
                .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_INVOKE, e.to_string()))?;
            let _ = std::fs::remove_file(&runner_endpoint);
            Ok(())
        }

        async fn stop_service0(&self) -> Result<(), ImagodError> {
            self.supervisor
                .stop(Self::service_name(ServiceAtom::Service0), true)
                .await
        }

        async fn reap_exited_service0(&self) -> Result<(), ImagodError> {
            stop_running_service_best_effort(
                &self.supervisor.inner,
                Self::service_name(ServiceAtom::Service0),
            )
            .await;
            let child = Command::new("sh")
                .arg("-c")
                .arg("exit 0")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| ImagodError::new(ErrorCode::Internal, STAGE_STOP, e.to_string()))?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            let service = new_running_service(
                child,
                Self::runner_id(ServiceAtom::Service0),
                self.root
                    .join("runtime")
                    .join("ipc")
                    .join("runner-source-exited.sock"),
            );
            {
                let mut guard = self.supervisor.inner.write().await;
                guard.insert(
                    Self::service_name(ServiceAtom::Service0).to_string(),
                    service,
                );
            }
            self.supervisor.reap_finished().await;
            Ok(())
        }

        async fn shutdown_drain(&self) -> Result<(), ImagodError> {
            let errors = self.supervisor.stop_all(true).await;
            if errors.is_empty() {
                Ok(())
            } else {
                let (service, err) = errors.into_iter().next().expect("errors should exist");
                Err(ImagodError::new(
                    err.code,
                    err.stage,
                    format!("shutdown drain failed for {service}: {}", err.message),
                ))
            }
        }
    }

    impl Drop for RuntimeProjectionRuntime {
        fn drop(&mut self) {
            if let Ok(inner) = self.supervisor.inner.try_read() {
                let pids = inner
                    .values()
                    .filter_map(|service| service.child.id())
                    .collect::<Vec<_>>();
                drop(inner);
                for pid in pids {
                    let _ = std::process::Command::new("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .status();
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl ProtocolRuntimeBinding<RuntimeProjectionSpec> for RuntimeProjectionBinding {
        type Runtime = RuntimeProjectionRuntime;
        type Context = ();

        async fn fresh_runtime(_spec: &RuntimeProjectionSpec) -> Self::Runtime {
            RuntimeProjectionRuntime::new().await
        }

        fn context(_spec: &RuntimeProjectionSpec) -> Self::Context {}
    }

    impl ProtocolActionApplier for RuntimeProjectionRuntime {
        type Action = RuntimeProjectionAction;
        type Output = Vec<SystemEffect>;
        type Context = ();

        async fn execute_action(
            &self,
            _context: &Self::Context,
            action: &Self::Action,
        ) -> Self::Output {
            let spec = RuntimeProjectionSpec::new();
            let prev = self.state.lock().await.clone();
            let Some(next) = spec.transition(&prev, action) else {
                return Vec::new();
            };

            let result = match action {
                RuntimeProjectionAction::DeployService0 => {
                    self.deploy_service(ServiceAtom::Service0).await
                }
                RuntimeProjectionAction::DeployService1 => {
                    self.deploy_service(ServiceAtom::Service1).await
                }
                RuntimeProjectionAction::RollbackService0 => self.rollback_service0().await,
                RuntimeProjectionAction::LocalRpcResolved => self.local_rpc_resolved().await,
                RuntimeProjectionAction::LocalRpcDenied => self.local_rpc_denied().await,
                RuntimeProjectionAction::RemoteRpcLifecycle => self.remote_rpc_lifecycle().await,
                RuntimeProjectionAction::StopService0 => self.stop_service0().await,
                RuntimeProjectionAction::ReapExitedService0 => self.reap_exited_service0().await,
                RuntimeProjectionAction::ShutdownDrain => self.shutdown_drain().await,
            };
            result.expect("runtime projection action should succeed");

            *self.state.lock().await = next.clone();
            self.trace.lock().await.push(*action);
            spec.expected_output(&prev, action, Some(&next))
        }
    }

    impl StateObserver for RuntimeProjectionRuntime {
        type ObservedState = RuntimeProjectionObservedState;
        type Context = ();

        async fn observe_state(&self, _context: &Self::Context) -> Self::ObservedState {
            RuntimeProjectionObservedState {
                trace: self.trace.lock().await.clone(),
            }
        }
    }

    #[code_tests(spec = RuntimeProjectionSpec, binding = RuntimeProjectionBinding)]
    const _: () = ();

    #[test]
    fn bounded_log_buffer_keeps_latest_bytes_only() {
        let mut buffer = log_buffer::BoundedLogEventBuffer::new(5);
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
        assert_eq!(buffer.total_bytes(), 5);
        assert_eq!(buffer.snapshot_bytes(), b"bcdef");
    }

    #[tokio::test]
    async fn effective_http_worker_queue_capacity_respects_budget_when_body_is_8mib() {
        let root = new_test_root("http-queue-budget-8mib");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize")
            .with_http_queue_memory_budget_bytes(32 * 1024 * 1024);
        let launch = sample_http_launch(8 * 1024 * 1024);
        assert_eq!(
            supervisor
                .effective_http_worker_queue_capacity(&launch)
                .expect("queue capacity should be computed"),
            4
        );

        drop(supervisor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn effective_http_worker_queue_capacity_clamps_to_budget_when_body_is_16mib() {
        let root = new_test_root("http-queue-budget-16mib");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize")
            .with_http_queue_memory_budget_bytes(32 * 1024 * 1024);
        let launch = sample_http_launch(16 * 1024 * 1024);
        assert_eq!(
            supervisor
                .effective_http_worker_queue_capacity(&launch)
                .expect("queue capacity should be clamped by budget"),
            2
        );

        drop(supervisor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn effective_http_worker_queue_capacity_rejects_when_budget_is_smaller_than_max_body() {
        let root = new_test_root("http-queue-budget-too-small");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize")
            .with_http_queue_memory_budget_bytes(32 * 1024 * 1024);
        let launch = sample_http_launch(64 * 1024 * 1024);
        let err = supervisor
            .effective_http_worker_queue_capacity(&launch)
            .expect_err("budget smaller than max_body should fail");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_START);
        assert!(
            err.message
                .contains("runtime.http_queue_memory_budget_bytes")
        );

        drop(supervisor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn attach_start_failure_wasm_log_details_adds_details_only_when_enabled() {
        let composite_log = Arc::new(Mutex::new(CompositeLogBuffer::new(256)));
        {
            let mut buffer = composite_log.lock().await;
            buffer.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"stdout-line\n".to_vec(),
                timestamp_unix_ms: 1,
            });
            buffer.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stderr,
                bytes: b"stderr-line\n".to_vec(),
                timestamp_unix_ms: 2,
            });
        }
        let log_buffers = StartFailureLogBuffers { composite_log };

        let enabled = attach_start_failure_wasm_log_details(
            ImagodError::new(ErrorCode::Internal, STAGE_START, "runner failed"),
            true,
            &log_buffers,
        )
        .await;
        assert_eq!(
            enabled.details.get(DETAIL_WASM_STDOUT).map(String::as_str),
            Some("stdout-line\n")
        );
        assert_eq!(
            enabled.details.get(DETAIL_WASM_STDERR).map(String::as_str),
            Some("stderr-line\n")
        );

        let disabled = attach_start_failure_wasm_log_details(
            ImagodError::new(ErrorCode::Internal, STAGE_START, "runner failed"),
            false,
            &log_buffers,
        )
        .await;
        assert!(!disabled.details.contains_key(DETAIL_WASM_STDOUT));
        assert!(!disabled.details.contains_key(DETAIL_WASM_STDERR));
    }

    #[tokio::test]
    async fn attach_start_failure_wasm_log_details_skips_empty_streams() {
        let log_buffers = StartFailureLogBuffers {
            composite_log: Arc::new(Mutex::new(CompositeLogBuffer::new(128))),
        };

        let err = attach_start_failure_wasm_log_details(
            ImagodError::new(ErrorCode::Internal, STAGE_START, "runner failed"),
            true,
            &log_buffers,
        )
        .await;
        assert!(err.details.is_empty());
    }

    #[test]
    fn tail_snapshot_bytes_from_events_returns_last_n_lines() {
        let events = vec![
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"l1\n".to_vec(),
                timestamp_unix_ms: 1,
            },
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"l2\n".to_vec(),
                timestamp_unix_ms: 2,
            },
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"l3\n".to_vec(),
                timestamp_unix_ms: 3,
            },
        ];
        assert_eq!(tail_snapshot_bytes_from_events(&events, 1), b"l3\n");
        assert_eq!(tail_snapshot_bytes_from_events(&events, 2), b"l2\nl3\n");
        assert_eq!(tail_snapshot_bytes_from_events(&events, 0), b"");
    }

    #[test]
    fn tail_snapshot_events_from_events_preserves_timestamp_for_tailed_content() {
        let events = vec![
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"l1\n".to_vec(),
                timestamp_unix_ms: 1,
            },
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"l2\n".to_vec(),
                timestamp_unix_ms: 2,
            },
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"l3\n".to_vec(),
                timestamp_unix_ms: 3,
            },
        ];

        let tailed = tail_snapshot_bytes_from_events(&events, 2);
        let tailed_events = tail_snapshot_events_from_events(&events, 2);
        let joined = snapshot_bytes_from_events(&tailed_events);
        let timestamps = tailed_events
            .iter()
            .map(|event| event.timestamp_unix_ms)
            .collect::<Vec<_>>();

        assert_eq!(joined, tailed);
        assert_eq!(timestamps, vec![2, 3]);
    }

    #[test]
    fn tail_snapshot_events_from_events_preserves_partial_event_timestamp() {
        let events = vec![ServiceLogEvent {
            stream: ServiceLogStream::Stdout,
            bytes: b"abc\ndef\n".to_vec(),
            timestamp_unix_ms: 10,
        }];

        let tailed_events = tail_snapshot_events_from_events(&events, 1);
        assert_eq!(snapshot_bytes_from_events(&tailed_events), b"def\n");
        assert_eq!(tailed_events.len(), 1);
        assert_eq!(tailed_events[0].timestamp_unix_ms, 10);
    }

    #[tokio::test]
    async fn retained_logs_capacity_is_double_runner_log_buffer_bytes() {
        let root = new_test_root("retained-capacity-double");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 1024, 50)
            .expect("supervisor should initialize");
        let capacity = supervisor.retained_logs.lock().await.capacity_bytes();
        assert_eq!(capacity, 2048);

        drop(supervisor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_retains_snapshot_with_events_under_double_capacity() {
        let root = new_test_root("retained-capacity-snapshot");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 1024, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-retained-capacity";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("retained-capacity.sock");

        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");
        let mut service = new_running_service(child, "runner-retained-capacity", runner_endpoint);
        service.composite_log = Arc::new(Mutex::new(CompositeLogBuffer::new(1024)));
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: vec![b'x'; 900],
                timestamp_unix_ms: 42,
            });
        }
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        supervisor
            .stop(service_name, true)
            .await
            .expect("force stop should succeed");
        let subscription = supervisor
            .open_logs(service_name, 10, false, false)
            .await
            .expect("retained logs should remain available");
        assert_eq!(expect_snapshot_bytes(&subscription.snapshot).len(), 900);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runner_endpoint_for_uses_fixed_length_hash_name() {
        let root = new_test_root("endpoint-hash");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

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

    #[tokio::test]
    async fn open_logs_returns_tail_snapshot_and_follow_receiver() {
        let root = new_test_root("open-logs");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-open-logs";
        let runner_endpoint = root.join("runtime").join("ipc").join("open-logs.sock");
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");

        let service = new_running_service(child, "runner-open-logs", runner_endpoint);
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"a\n".to_vec(),
                timestamp_unix_ms: 10,
            });
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"b\n".to_vec(),
                timestamp_unix_ms: 11,
            });
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"c\n".to_vec(),
                timestamp_unix_ms: 12,
            });
        }

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        let byte_subscription = supervisor
            .open_logs(service_name, 2, true, false)
            .await
            .expect("open_logs should succeed");
        assert_eq!(byte_subscription.service_name, service_name);
        assert_eq!(
            expect_snapshot_bytes(&byte_subscription.snapshot),
            b"b\nc\n"
        );
        assert!(
            byte_subscription.receiver.is_some(),
            "follow should subscribe"
        );

        let event_subscription = supervisor
            .open_logs(service_name, 2, true, true)
            .await
            .expect("timestamped open_logs should succeed");
        assert_eq!(
            snapshot_bytes_from_events(expect_snapshot_events(&event_subscription.snapshot)),
            b"b\nc\n"
        );
        assert_eq!(
            expect_snapshot_events(&event_subscription.snapshot)
                .iter()
                .map(|event| event.timestamp_unix_ms)
                .collect::<Vec<_>>(),
            vec![11, 12]
        );
        assert!(
            event_subscription.receiver.is_some(),
            "follow should subscribe"
        );

        stop_running_service_best_effort(&supervisor.inner, service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn open_logs_subscribes_before_snapshot_read_for_follow() {
        let root = new_test_root("open-logs-order");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-open-logs-order";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("open-logs-order.sock");
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");

        let service = new_running_service(child, "runner-open-logs-order", runner_endpoint);
        let composite_log = service.composite_log.clone();
        let log_sender = service.log_sender.clone();
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        let held_snapshot_lock = composite_log.lock().await;
        let open_task = {
            let supervisor = supervisor.clone();
            tokio::spawn(async move { supervisor.open_logs(service_name, 10, true, false).await })
        };

        time::timeout(Duration::from_millis(200), async {
            while log_sender.receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("follow receiver should be registered before snapshot lock is released");

        drop(held_snapshot_lock);
        let subscription = open_task
            .await
            .expect("open_logs task should complete")
            .expect("open_logs should succeed");
        assert!(subscription.receiver.is_some(), "follow should subscribe");

        stop_running_service_best_effort(&supervisor.inner, service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn open_logs_returns_retained_snapshot_with_no_follow_receiver() {
        let root = new_test_root("open-logs-retained");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-open-logs-retained";

        supervisor
            .retained_logs
            .lock()
            .await
            .upsert(
                service_name,
                &[
                    ServiceLogEvent {
                        stream: ServiceLogStream::Stdout,
                        bytes: b"old-a\n".to_vec(),
                        timestamp_unix_ms: 21,
                    },
                    ServiceLogEvent {
                        stream: ServiceLogStream::Stdout,
                        bytes: b"old-b\n".to_vec(),
                        timestamp_unix_ms: 22,
                    },
                    ServiceLogEvent {
                        stream: ServiceLogStream::Stdout,
                        bytes: b"old-c\n".to_vec(),
                        timestamp_unix_ms: 23,
                    },
                ],
            )
            .expect("retained upsert should succeed");

        let byte_subscription = supervisor
            .open_logs(service_name, 2, true, false)
            .await
            .expect("retained open_logs should succeed");
        assert_eq!(byte_subscription.service_name, service_name);
        assert_eq!(
            expect_snapshot_bytes(&byte_subscription.snapshot),
            b"old-b\nold-c\n"
        );
        assert!(
            byte_subscription.receiver.is_none(),
            "retained logs should not provide follow receiver"
        );

        let event_subscription = supervisor
            .open_logs(service_name, 2, true, true)
            .await
            .expect("timestamped retained open_logs should succeed");
        assert_eq!(
            snapshot_bytes_from_events(expect_snapshot_events(&event_subscription.snapshot)),
            b"old-b\nold-c\n"
        );
        assert_eq!(
            expect_snapshot_events(&event_subscription.snapshot)
                .iter()
                .map(|event| event.timestamp_unix_ms)
                .collect::<Vec<_>>(),
            vec![22, 23]
        );
        assert!(event_subscription.receiver.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn open_logs_does_not_return_retained_snapshot_while_service_is_stopping() {
        let root = new_test_root("open-logs-retained-stopping");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-open-logs-retained-stopping";

        supervisor
            .retained_logs
            .lock()
            .await
            .upsert(service_name, &[])
            .expect("retained upsert should succeed");

        let _stopping_guard = supervisor.begin_stop_service(service_name);
        let err = supervisor
            .open_logs(service_name, 10, true, false)
            .await
            .expect_err("stopping service should not serve stale retained logs");
        assert_eq!(err.code, ErrorCode::NotFound);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn open_logs_prefers_running_snapshot_over_retained_snapshot() {
        let root = new_test_root("open-logs-priority");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-open-logs-priority";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("open-logs-priority.sock");
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");

        let service = new_running_service(child, "runner-open-logs-priority", runner_endpoint);
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"running-a\nrunning-b\n".to_vec(),
                timestamp_unix_ms: 1,
            });
        }

        supervisor
            .retained_logs
            .lock()
            .await
            .upsert(service_name, &[])
            .expect("retained upsert should succeed");
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        let subscription = supervisor
            .open_logs(service_name, 10, true, false)
            .await
            .expect("open_logs should prefer running service");
        assert_eq!(
            expect_snapshot_bytes(&subscription.snapshot),
            b"running-a\nrunning-b\n"
        );
        assert!(
            subscription.receiver.is_some(),
            "running logs should provide follow receiver"
        );

        stop_running_service_best_effort(&supervisor.inner, service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn running_service_names_returns_running_only() {
        let root = new_test_root("running-names");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

        let endpoint_a = root.join("runtime").join("ipc").join("running-a.sock");
        let endpoint_b = root.join("runtime").join("ipc").join("running-b.sock");
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

        let service_a = new_running_service(child_a, "runner-running-a", endpoint_a);
        let mut service_b = new_running_service(child_b, "runner-running-b", endpoint_b);
        service_b.status = RunningStatus::Stopping;

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert("svc-running-a".to_string(), service_a);
            inner.insert("svc-running-b".to_string(), service_b);
        }

        let mut names = supervisor.running_service_names().await;
        names.sort();
        assert_eq!(names, vec!["svc-running-a".to_string()]);

        stop_running_service_best_effort(&supervisor.inner, "svc-running-a").await;
        stop_running_service_best_effort(&supervisor.inner, "svc-running-b").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_service_states_returns_running_and_stopping_states() {
        let root = new_test_root("runtime-states");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

        let endpoint_a = root.join("runtime").join("ipc").join("runtime-a.sock");
        let endpoint_b = root.join("runtime").join("ipc").join("runtime-b.sock");
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

        let mut service_a = new_running_service(child_a, "runner-runtime-a", endpoint_a);
        service_a.release_hash = "release-runtime-a".to_string();
        service_a.started_at = "111".to_string();
        let mut service_b = new_running_service(child_b, "runner-runtime-b", endpoint_b);
        service_b.release_hash = "release-runtime-b".to_string();
        service_b.started_at = "222".to_string();
        service_b.status = RunningStatus::Stopping;

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert("svc-runtime-a".to_string(), service_a);
            inner.insert("svc-runtime-b".to_string(), service_b);
        }

        let states = supervisor.runtime_service_states().await;
        assert_eq!(
            states,
            vec![
                RuntimeServiceState {
                    name: "svc-runtime-a".to_string(),
                    release_hash: "release-runtime-a".to_string(),
                    started_at: "111".to_string(),
                    status: RunningStatus::Running,
                },
                RuntimeServiceState {
                    name: "svc-runtime-b".to_string(),
                    release_hash: "release-runtime-b".to_string(),
                    started_at: "222".to_string(),
                    status: RunningStatus::Stopping,
                }
            ]
        );

        stop_running_service_best_effort(&supervisor.inner, "svc-runtime-a").await;
        stop_running_service_best_effort(&supervisor.inner, "svc-runtime-b").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn loggable_service_names_merges_running_and_retained_without_duplicates() {
        let root = new_test_root("loggable-names");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

        let endpoint = root
            .join("runtime")
            .join("ipc")
            .join("loggable-running.sock");
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("running child should spawn");
        let service = new_running_service(child, "runner-loggable-running", endpoint);
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert("svc-running".to_string(), service);
        }
        {
            let mut retained = supervisor.retained_logs.lock().await;
            retained
                .upsert(
                    "svc-running",
                    &[ServiceLogEvent {
                        stream: ServiceLogStream::Stdout,
                        bytes: b"running-retained\n".to_vec(),
                        timestamp_unix_ms: 1,
                    }],
                )
                .expect("retained upsert should succeed");
            retained
                .upsert(
                    "svc-retained",
                    &[ServiceLogEvent {
                        stream: ServiceLogStream::Stdout,
                        bytes: b"retained-only\n".to_vec(),
                        timestamp_unix_ms: 2,
                    }],
                )
                .expect("retained upsert should succeed");
        }

        let names = supervisor.loggable_service_names().await;
        assert_eq!(
            names,
            vec!["svc-retained".to_string(), "svc-running".to_string()]
        );

        stop_running_service_best_effort(&supervisor.inner, "svc-running").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn loggable_service_names_excludes_services_being_stopped() {
        let root = new_test_root("loggable-names-stopping");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

        {
            let mut retained = supervisor.retained_logs.lock().await;
            retained
                .upsert(
                    "svc-stopping",
                    &[ServiceLogEvent {
                        stream: ServiceLogStream::Stdout,
                        bytes: b"retained\n".to_vec(),
                        timestamp_unix_ms: 1,
                    }],
                )
                .expect("retained upsert should succeed");
            retained
                .upsert(
                    "svc-available",
                    &[ServiceLogEvent {
                        stream: ServiceLogStream::Stdout,
                        bytes: b"retained\n".to_vec(),
                        timestamp_unix_ms: 2,
                    }],
                )
                .expect("retained upsert should succeed");
        }
        let _stopping_guard = supervisor.begin_stop_service("svc-stopping");

        let names = supervisor.loggable_service_names().await;
        assert_eq!(names, vec!["svc-available".to_string()]);

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

    #[test]
    fn manager_control_endpoint_path_too_long_is_rejected_before_bind() {
        let root = PathBuf::from(format!("/tmp/iss-control-too-long-{}", "x".repeat(90)));
        let _ = std::fs::remove_dir_all(&root);

        let err = match ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50) {
            Ok(_) => panic!("too long manager control endpoint should be rejected"),
            Err(err) => err,
        };
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message
                .contains("manager control endpoint path is too long for AF_UNIX"),
            "unexpected error message: {}",
            err.message
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn reserve_start_rejects_duplicate_service_name() {
        let root = new_test_root("start-reserve");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

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

    #[tokio::test]
    async fn reserve_start_reclaims_exited_service_before_busy_check() {
        let root = new_test_root("start-reserve-reclaim-exited");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-reserve-reclaim";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("reserve-reclaim.sock");
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
        let service = new_running_service(child, "runner-reserve-reclaim", runner_endpoint.clone());
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"reclaim-a\nreclaim-b\n".to_vec(),
                timestamp_unix_ms: 1,
            });
        }
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        supervisor
            .reserve_start(service_name)
            .await
            .expect("stale exited service should be reclaimed before busy check");
        assert!(
            !runner_endpoint.exists(),
            "stale runner endpoint should be removed after reclaim"
        );
        let inner = supervisor.inner.read().await;
        assert!(
            !inner.contains_key(service_name),
            "exited service should be removed from running map"
        );
        drop(inner);

        let retained = supervisor
            .open_logs(service_name, 10, false, false)
            .await
            .expect("reclaimed service logs should be retained");
        assert_eq!(
            expect_snapshot_bytes(&retained.snapshot),
            b"reclaim-a\nreclaim-b\n"
        );
        assert!(retained.receiver.is_none());

        supervisor.release_start(service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bindings_allow_target_and_wit_pair_only() {
        let bindings = vec![ServiceBinding {
            name: "svc-b".to_string(),
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

    #[test]
    fn manager_auth_validation_accepts_correct_proof() {
        let secret = random_secret_hex();
        let proof =
            compute_manager_auth_proof(&secret, "runner-1").expect("proof should be generated");
        validate_manager_auth(&secret, "runner-1", &proof)
            .expect("proof validation should succeed");
    }

    #[tokio::test]
    async fn resolve_invocation_target_rejects_target_before_runner_ready() {
        let root = new_test_root("resolve-not-ready");
        let inner = Arc::new(RwLock::new(BTreeMap::new()));
        let pending_ready = Arc::new(Mutex::new(BTreeMap::new()));
        let source_service_name = "svc-source".to_string();
        let target_service_name = "svc-target".to_string();
        let source_runner_id = "runner-source";
        let target_runner_id = "runner-target";
        let wit = "pkg:iface/invoke".to_string();
        let source_endpoint = root.join("source.sock");
        let target_endpoint = root.join("target.sock");

        let source_child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("source child should spawn");
        let target_child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("target child should spawn");

        let mut source = new_running_service(source_child, source_runner_id, source_endpoint);
        source.bindings = vec![ServiceBinding {
            name: target_service_name.clone(),
            wit: wit.clone(),
        }];
        let source_secret = source.manager_auth_secret.clone();

        let mut target = new_running_service(target_child, target_runner_id, target_endpoint);
        target.is_ready = false;

        {
            let mut guard = inner.write().await;
            guard.insert(source_service_name.clone(), source);
            guard.insert(target_service_name.clone(), target);
        }

        let manager_auth_proof = compute_manager_auth_proof(&source_secret, source_runner_id)
            .expect("proof should be generated");
        let response = handle_control_request(
            &inner,
            &pending_ready,
            ControlRequest::ResolveInvocationTarget {
                runner_id: source_runner_id.to_string(),
                manager_auth_proof,
                target_service: target_service_name.clone(),
                wit: wit.clone(),
            },
        )
        .await;

        match response {
            ControlResponse::Error(err) => {
                assert_eq!(err.code, ErrorCode::NotFound);
                assert_eq!(err.message, "target service is not running");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        stop_running_service_best_effort(&inner, &source_service_name).await;
        stop_running_service_best_effort(&inner, &target_service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn resolve_invocation_target_accepts_target_after_runner_ready() {
        let root = new_test_root("resolve-ready");
        let inner = Arc::new(RwLock::new(BTreeMap::new()));
        let pending_ready = Arc::new(Mutex::new(BTreeMap::new()));
        let source_service_name = "svc-source".to_string();
        let target_service_name = "svc-target".to_string();
        let source_runner_id = "runner-source";
        let target_runner_id = "runner-target";
        let wit = "pkg:iface/invoke".to_string();
        let source_endpoint = root.join("source.sock");
        let target_endpoint = root.join("target.sock");

        let source_child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("source child should spawn");
        let target_child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("target child should spawn");

        let mut source = new_running_service(source_child, source_runner_id, source_endpoint);
        source.bindings = vec![ServiceBinding {
            name: target_service_name.clone(),
            wit: wit.clone(),
        }];
        let source_secret = source.manager_auth_secret.clone();

        let mut target =
            new_running_service(target_child, target_runner_id, target_endpoint.clone());
        target.is_ready = false;
        let target_secret = target.manager_auth_secret.clone();
        let target_invocation_secret = target.invocation_secret.clone();

        {
            let mut guard = inner.write().await;
            guard.insert(source_service_name.clone(), source);
            guard.insert(target_service_name.clone(), target);
        }

        let target_ready_proof = compute_manager_auth_proof(&target_secret, target_runner_id)
            .expect("target ready proof should be generated");
        let ready_response = handle_control_request(
            &inner,
            &pending_ready,
            ControlRequest::RunnerReady {
                runner_id: target_runner_id.to_string(),
                manager_auth_proof: target_ready_proof,
            },
        )
        .await;
        assert!(
            matches!(ready_response, ControlResponse::Ack),
            "runner ready should be accepted"
        );

        let manager_auth_proof = compute_manager_auth_proof(&source_secret, source_runner_id)
            .expect("source proof should be generated");
        let response = handle_control_request(
            &inner,
            &pending_ready,
            ControlRequest::ResolveInvocationTarget {
                runner_id: source_runner_id.to_string(),
                manager_auth_proof,
                target_service: target_service_name.clone(),
                wit: wit.clone(),
            },
        )
        .await;

        match response {
            ControlResponse::ResolvedInvocationTarget { endpoint, token } => {
                assert_eq!(endpoint, target_endpoint);
                let claims = imagod_ipc::verify_invocation_token(&target_invocation_secret, &token)
                    .expect("returned invocation token should verify");
                assert_eq!(claims.source_service, source_service_name);
                assert_eq!(claims.target_service, target_service_name);
                assert_eq!(claims.wit, wit);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        stop_running_service_best_effort(&inner, "svc-source").await;
        stop_running_service_best_effort(&inner, "svc-target").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn invoke_returns_not_found_when_target_is_not_ready() {
        let root = new_test_root("invoke-not-ready");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let target_service_name = "svc-target";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("invoke-not-ready.sock");
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("target child should spawn");

        let mut target = new_running_service(child, "runner-invoke-not-ready", runner_endpoint);
        target.is_ready = false;
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(target_service_name.to_string(), target);
        }

        let err = supervisor
            .invoke(
                target_service_name,
                "yieldspace:service/invoke",
                "call",
                Vec::new(),
            )
            .await
            .expect_err("not-ready target should be rejected");
        assert_eq!(err.code, ErrorCode::NotFound);

        stop_running_service_best_effort(&supervisor.inner, target_service_name).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn invoke_issues_remote_token_and_returns_runner_payload() {
        let root = new_test_root("invoke-success");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let target_service_name = "svc-target";
        let runner_endpoint = root.join("runtime").join("ipc").join("invoke-success.sock");
        if let Some(parent) = runner_endpoint.parent() {
            std::fs::create_dir_all(parent).expect("runner endpoint parent should be created");
        }
        let _ = std::fs::remove_file(&runner_endpoint);
        let listener = UnixListener::bind(&runner_endpoint).expect("runner listener should bind");

        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("target child should spawn");
        let target = new_running_service(child, "runner-invoke-success", runner_endpoint.clone());
        let target_invocation_secret = target.invocation_secret.clone();
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(target_service_name.to_string(), target);
        }

        let server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept should succeed");
            let request = DbusP2pTransport::read_message::<RunnerInboundRequest>(&mut stream)
                .await
                .expect("invoke request should decode");
            match request {
                RunnerInboundRequest::Invoke {
                    interface_id,
                    function,
                    payload_cbor,
                    token,
                } => {
                    assert_eq!(interface_id, "yieldspace:service/invoke");
                    assert_eq!(function, "call");
                    let claims =
                        imagod_ipc::verify_invocation_token(&target_invocation_secret, &token)
                            .expect("token should verify");
                    assert_eq!(claims.source_service, "remote");
                    assert_eq!(claims.target_service, "svc-target");
                    assert_eq!(claims.wit, "yieldspace:service/invoke");

                    DbusP2pTransport::write_message(
                        &mut stream,
                        &RunnerInboundResponse::InvokeResult { payload_cbor },
                    )
                    .await
                    .expect("invoke response should write");
                }
                other => panic!("unexpected request: {other:?}"),
            }
        });

        let payload = vec![0x01, 0x02, 0x03];
        let result = supervisor
            .invoke(
                target_service_name,
                "yieldspace:service/invoke",
                "call",
                payload.clone(),
            )
            .await
            .expect("invoke should succeed");
        assert_eq!(result, payload);

        server_task.await.expect("server task should complete");
        stop_running_service_best_effort(&supervisor.inner, target_service_name).await;
        let _ = std::fs::remove_file(&runner_endpoint);
        let _ = std::fs::remove_dir_all(&root);
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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

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
    async fn stop_removes_runner_index_entry() {
        let root = new_test_root("stop-runner-index");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-stop-index";
        let runner_id = "runner-stop-index";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-index.sock");

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
        supervisor
            .runner_index
            .write()
            .await
            .insert(runner_id.to_string(), service_name.to_string());

        supervisor
            .stop(service_name, true)
            .await
            .expect("force stop should succeed");
        assert!(
            !supervisor.runner_index.read().await.contains_key(runner_id),
            "runner index entry should be removed after stop"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_success_registers_retained_snapshot() {
        let root = new_test_root("stop-retained-success");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-stop-retained-success";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-retained-success.sock");

        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep process should spawn");
        let service = new_running_service(child, "runner-stop-retained-success", runner_endpoint);
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"line-a\nline-b\n".to_vec(),
                timestamp_unix_ms: 1,
            });
        }
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        supervisor
            .stop(service_name, true)
            .await
            .expect("force stop should succeed");
        let subscription = supervisor
            .open_logs(service_name, 10, true, false)
            .await
            .expect("retained logs should be available");
        assert_eq!(
            expect_snapshot_bytes(&subscription.snapshot),
            b"line-a\nline-b\n"
        );
        assert!(
            subscription.receiver.is_none(),
            "retained logs should not include follow receiver"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_already_exited_registers_retained_snapshot() {
        let root = new_test_root("stop-retained-already-exited");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-stop-retained-already-exited";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-retained-already-exited.sock");

        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child process should spawn");
        let service = new_running_service(
            child,
            "runner-stop-retained-already-exited",
            runner_endpoint,
        );
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"done-a\ndone-b\n".to_vec(),
                timestamp_unix_ms: 1,
            });
        }
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let err = supervisor
            .stop(service_name, false)
            .await
            .expect_err("already exited service should return NotFound");
        assert_eq!(err.code, ErrorCode::NotFound);

        let subscription = supervisor
            .open_logs(service_name, 10, true, false)
            .await
            .expect("retained logs should remain available");
        assert_eq!(
            expect_snapshot_bytes(&subscription.snapshot),
            b"done-a\ndone-b\n"
        );
        assert!(subscription.receiver.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_failure_reinserts_service_into_supervisor_state() {
        let root = new_test_root("stop-reinsert");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("stop-reinsert.sock");
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
        let pid = child.id().expect("child pid should be available");

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                "svc-stop-reinsert".to_string(),
                new_running_service(child, "runner-stop-reinsert", runner_endpoint.clone()),
            );
        }

        inject_kill_and_wait_failure_for_pid(pid);
        let err = supervisor
            .stop("svc-stop-reinsert", true)
            .await
            .expect_err("forced stop should surface injected failure");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            runner_endpoint.exists(),
            "runner endpoint should remain on failed stop"
        );

        {
            let inner = supervisor.inner.read().await;
            let service = inner
                .get("svc-stop-reinsert")
                .expect("service should be reinserted after stop failure");
            assert_eq!(service.status, RunningStatus::Running);
        }

        let retry = supervisor.stop("svc-stop-reinsert", true).await;
        assert!(retry.is_ok(), "second force stop should succeed");
        assert!(
            !runner_endpoint.exists(),
            "runner endpoint should be removed after successful stop"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_recovery_terminates_displaced_child_when_service_name_is_taken() {
        let root = new_test_root("stop-recovery-displaced");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let displaced_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("displaced-old.sock");
        let active_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("runners")
            .join("displaced-new.sock");
        if let Some(parent) = displaced_endpoint.parent() {
            std::fs::create_dir_all(parent).expect("runner endpoint parent should be created");
        }
        std::fs::write(&displaced_endpoint, b"stale")
            .expect("displaced endpoint fixture should be created");

        let displaced_child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("displaced child should spawn");
        let displaced_pid = displaced_child
            .id()
            .expect("displaced pid should be available");
        let active_child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("active child should spawn");

        let displaced_service = new_running_service(
            displaced_child,
            "runner-displaced",
            displaced_endpoint.clone(),
        );
        let active_service =
            new_running_service(active_child, "runner-active", active_endpoint.clone());
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert("svc-displaced".to_string(), active_service);
        }

        supervisor
            .restore_service_after_stop_error("svc-displaced", displaced_service)
            .await;

        {
            let inner = supervisor.inner.read().await;
            let existing = inner
                .get("svc-displaced")
                .expect("existing service should remain tracked");
            assert_eq!(existing.runner_id, "runner-active");
            assert_eq!(existing.runner_endpoint, active_endpoint);
        }
        assert!(
            !displaced_endpoint.exists(),
            "displaced endpoint should be cleaned up after forced termination"
        );

        let process_exists = Command::new("kill")
            .arg("-0")
            .arg(displaced_pid.to_string())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("kill probe should run")
            .success();
        assert!(
            !process_exists,
            "displaced child should be terminated when reinsertion is skipped"
        );

        stop_running_service_best_effort(&supervisor.inner, "svc-displaced").await;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stop_all_stops_multiple_services_in_parallel() {
        let root = new_test_root("stop-all");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
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
    async fn reap_finished_registers_retained_snapshot() {
        let root = new_test_root("reap-retained");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-reap-retained";
        let runner_endpoint = root.join("runtime").join("ipc").join("reap-retained.sock");

        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child process should spawn");
        let service = new_running_service(child, "runner-reap-retained", runner_endpoint);
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"reap-a\nreap-b\n".to_vec(),
                timestamp_unix_ms: 1,
            });
        }
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        supervisor.reap_finished().await;

        let subscription = supervisor
            .open_logs(service_name, 10, true, false)
            .await
            .expect("reap should retain logs");
        assert_eq!(
            expect_snapshot_bytes(&subscription.snapshot),
            b"reap-a\nreap-b\n"
        );
        assert!(subscription.receiver.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn reap_finished_service_registers_retained_snapshot() {
        let root = new_test_root("reap-single-retained");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-reap-single-retained";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("reap-single-retained.sock");

        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child process should spawn");
        let service = new_running_service(
            child,
            "runner-reap-single-retained",
            runner_endpoint.clone(),
        );
        {
            let mut log = service.composite_log.lock().await;
            log.push_event(ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: b"single-a\nsingle-b\n".to_vec(),
                timestamp_unix_ms: 1,
            });
        }
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        supervisor.reap_finished_service(service_name).await;

        let subscription = supervisor
            .open_logs(service_name, 10, false, false)
            .await
            .expect("single-service reap should retain logs");
        assert_eq!(
            expect_snapshot_bytes(&subscription.snapshot),
            b"single-a\nsingle-b\n"
        );
        assert!(subscription.receiver.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn reap_finished_service_removes_runner_index_entry() {
        let root = new_test_root("reap-single-index");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let service_name = "svc-reap-single-index";
        let runner_id = "runner-reap-single-index";
        let runner_endpoint = root
            .join("runtime")
            .join("ipc")
            .join("reap-single-index.sock");

        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("child process should spawn");
        let service = new_running_service(child, runner_id, runner_endpoint);
        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(service_name.to_string(), service);
        }
        supervisor
            .runner_index
            .write()
            .await
            .insert(runner_id.to_string(), service_name.to_string());

        tokio::time::sleep(Duration::from_millis(50)).await;
        supervisor.reap_finished_service(service_name).await;

        assert!(
            !supervisor.runner_index.read().await.contains_key(runner_id),
            "runner index entry should be removed after single-service reap"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn manager_control_idle_connection_does_not_block_next_request() {
        let root = new_test_root("control-parallel");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");

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
    async fn manager_control_server_limits_concurrent_handlers() {
        let root = new_test_root("control-limit");
        let supervisor = ServiceSupervisor::new(&root, 1, 5, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
        let mut idle_connections = Vec::new();

        for _ in 0..MAX_MANAGER_CONTROL_CONNECTION_HANDLERS {
            let stream = tokio::net::UnixStream::connect(&supervisor.manager_control_endpoint)
                .await
                .expect("idle connection should open");
            idle_connections.push(stream);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let endpoint = supervisor.manager_control_endpoint.clone();
        let request_task = tokio::spawn(async move {
            DbusP2pTransport::call_control(
                &endpoint,
                &ControlRequest::Heartbeat {
                    runner_id: "missing-runner".to_string(),
                    manager_auth_proof: "proof".to_string(),
                },
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !request_task.is_finished(),
            "request should wait while all handler permits are consumed"
        );

        drop(idle_connections.pop());
        let response = tokio::time::timeout(Duration::from_secs(2), request_task)
            .await
            .expect("request should complete after one permit is released")
            .expect("request task should join")
            .expect("call_control should return response");
        match response {
            ControlResponse::Error(err) => assert_eq!(err.code, ErrorCode::NotFound),
            other => panic!("unexpected response: {other:?}"),
        }

        drop(idle_connections);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn dropping_supervisor_removes_manager_control_endpoint() {
        let root = new_test_root("control-cleanup");
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
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
        let supervisor = ServiceSupervisor::new(&root, 1, 1, 1_000, 2, 4, 4096, 50)
            .expect("supervisor should initialize");
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
