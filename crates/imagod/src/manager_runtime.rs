use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use imagod_common::ImagodError;
use imagod_config::{ImagodConfig, load_or_create_default, resolve_config_path};
use imagod_control::{ArtifactStore, OperationManager, Orchestrator, ServiceSupervisor};
use imagod_server::{ProtocolHandler, build_server};
use web_transport_quinn::http::StatusCode;

use crate::shutdown::{
    drain_session_tasks, log_session_task_join_result, wait_for_maintenance_shutdown,
};

const SESSION_TASK_DRAIN_TIMEOUT_SECS: u64 = 15;
const IDLE_MAINTENANCE_TICK_SECS: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ManagerRuntimeTaskState {
    #[default]
    NotStarted,
    Succeeded,
    Failed,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ManagerRuntimeTaskKind {
    #[default]
    PluginGc,
    BootRestore,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ManagerRuntimeShutdownPhase {
    #[default]
    Idle,
    SignalReceived,
    DrainingSessions,
    StoppingServices,
    StoppingMaintenance,
    Completed,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ManagerRuntimePhase {
    #[default]
    Booting,
    ConfigReady,
    Restoring,
    Listening,
    ShutdownRequested,
    Stopped,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ManagerRuntimeShutdownState {
    pub phase: ManagerRuntimeShutdownPhase,
    pub accepts_stopped: bool,
    pub sessions_drained: bool,
    pub services_stopped: bool,
    pub maintenance_stopped: bool,
    pub forced_stop_attempted: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagerRuntimeSnapshot {
    pub config_loaded: bool,
    pub created_default: bool,
    pub manager_phase: ManagerRuntimePhase,
    pub listening: bool,
    pub session_shutdown_requested: bool,
    pub shutdown: ManagerRuntimeShutdownState,
}

impl Default for ManagerRuntimeSnapshot {
    fn default() -> Self {
        Self {
            config_loaded: false,
            created_default: false,
            manager_phase: ManagerRuntimePhase::Booting,
            listening: false,
            session_shutdown_requested: false,
            shutdown: ManagerRuntimeShutdownState::default(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagerRuntimeEffect {
    TaskMilestone(ManagerRuntimeTaskKind, ManagerRuntimeTaskState),
    ShutdownComplete,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
struct ManagerRuntimeObservationState {
    snapshot: ManagerRuntimeSnapshot,
    effects: Vec<ManagerRuntimeEffect>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(crate) struct ManagerRuntimeObservation {
    inner: Arc<Mutex<ManagerRuntimeObservationState>>,
}

#[allow(dead_code)]
impl ManagerRuntimeObservation {
    pub(crate) fn snapshot(&self) -> ManagerRuntimeSnapshot {
        self.inner
            .lock()
            .expect("manager runtime observation should not be poisoned")
            .snapshot
    }

    pub(crate) fn drain_effects(&self) -> Vec<ManagerRuntimeEffect> {
        let mut guard = self
            .inner
            .lock()
            .expect("manager runtime observation should not be poisoned");
        std::mem::take(&mut guard.effects)
    }

    fn update_snapshot(&self, apply: impl FnOnce(&mut ManagerRuntimeSnapshot)) {
        let mut guard = self
            .inner
            .lock()
            .expect("manager runtime observation should not be poisoned");
        apply(&mut guard.snapshot);
    }

    fn push_effect(&self, effect: ManagerRuntimeEffect) {
        let mut guard = self
            .inner
            .lock()
            .expect("manager runtime observation should not be poisoned");
        guard.effects.push(effect);
    }
}

trait ManagerRuntimeObserver: Clone {
    fn note_config_loaded(&self, _created_default: bool) {}

    fn note_plugin_gc(&self, _state: ManagerRuntimeTaskState) {}

    fn note_boot_restore(&self, _state: ManagerRuntimeTaskState) {}

    fn note_listening(&self) {}

    fn note_shutdown_started(&self) {}

    fn note_services_stopped(&self, _forced: bool) {}

    fn note_forced_stop_used(&self) {}

    fn note_maintenance_stopped(&self) {}

    fn note_shutdown_completed(&self) {}
}

#[derive(Debug, Clone, Default)]
struct NoopManagerRuntimeObserver;

impl ManagerRuntimeObserver for NoopManagerRuntimeObserver {}

impl ManagerRuntimeObserver for ManagerRuntimeObservation {
    fn note_config_loaded(&self, created_default: bool) {
        self.update_snapshot(|snapshot| {
            snapshot.config_loaded = true;
            snapshot.created_default = created_default;
            snapshot.manager_phase = ManagerRuntimePhase::ConfigReady;
        });
    }

    fn note_plugin_gc(&self, state: ManagerRuntimeTaskState) {
        self.update_snapshot(|snapshot| {
            snapshot.manager_phase = ManagerRuntimePhase::Restoring;
        });
        self.push_effect(ManagerRuntimeEffect::TaskMilestone(
            ManagerRuntimeTaskKind::PluginGc,
            state,
        ));
    }

    fn note_boot_restore(&self, state: ManagerRuntimeTaskState) {
        self.update_snapshot(|snapshot| {
            snapshot.manager_phase = ManagerRuntimePhase::Listening;
        });
        self.push_effect(ManagerRuntimeEffect::TaskMilestone(
            ManagerRuntimeTaskKind::BootRestore,
            state,
        ));
    }

    fn note_listening(&self) {
        self.update_snapshot(|snapshot| {
            snapshot.listening = true;
            snapshot.manager_phase = ManagerRuntimePhase::Listening;
        });
    }

    fn note_shutdown_started(&self) {
        self.update_snapshot(|snapshot| {
            snapshot.listening = false;
            snapshot.session_shutdown_requested = true;
            snapshot.manager_phase = ManagerRuntimePhase::ShutdownRequested;
            snapshot.shutdown.phase = ManagerRuntimeShutdownPhase::DrainingSessions;
            snapshot.shutdown.accepts_stopped = true;
        });
    }

    fn note_services_stopped(&self, forced: bool) {
        self.update_snapshot(|snapshot| {
            snapshot.shutdown.phase = ManagerRuntimeShutdownPhase::StoppingMaintenance;
            snapshot.shutdown.sessions_drained = true;
            snapshot.shutdown.services_stopped = true;
            snapshot.shutdown.forced_stop_attempted |= forced;
        });
    }

    fn note_forced_stop_used(&self) {
        self.update_snapshot(|snapshot| {
            snapshot.shutdown.forced_stop_attempted = true;
        });
    }

    fn note_maintenance_stopped(&self) {
        self.update_snapshot(|snapshot| {
            snapshot.shutdown.phase = ManagerRuntimeShutdownPhase::StoppingMaintenance;
            snapshot.shutdown.maintenance_stopped = true;
        });
    }

    fn note_shutdown_completed(&self) {
        self.update_snapshot(|snapshot| {
            snapshot.manager_phase = ManagerRuntimePhase::Stopped;
            snapshot.listening = false;
            snapshot.shutdown.phase = ManagerRuntimeShutdownPhase::Completed;
        });
        self.push_effect(ManagerRuntimeEffect::ShutdownComplete);
    }
}

struct BootContext {
    handler: ProtocolHandler,
    server: web_transport_quinn::Server,
    maintenance: MaintenanceLoop,
    session_concurrency_limit: usize,
}

struct MaintenanceLoop {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

struct AcceptLoopExit {
    session_tasks: tokio::task::JoinSet<()>,
    shutdown_started: bool,
}

pub(crate) async fn run_manager(config_path: Option<PathBuf>) -> Result<(), anyhow::Error> {
    run_manager_with_observer(config_path, NoopManagerRuntimeObserver).await
}

async fn run_manager_with_observer<O>(
    config_path: Option<PathBuf>,
    observer: O,
) -> Result<(), anyhow::Error>
where
    O: ManagerRuntimeObserver + Send + Sync + 'static,
{
    let mut boot = boot_manager(config_path, &observer).await?;
    let accept_exit = run_accept_loop(
        &mut boot.server,
        boot.handler.clone(),
        boot.session_concurrency_limit,
        &observer,
    )
    .await;
    shutdown_manager(boot.handler, boot.maintenance, accept_exit, &observer).await
}

async fn boot_manager<O>(
    config_path: Option<PathBuf>,
    observer: &O,
) -> Result<BootContext, anyhow::Error>
where
    O: ManagerRuntimeObserver + Send + Sync + 'static,
{
    let config_path = resolve_config_path(config_path);
    let load_result = load_or_create_default(&config_path).map_err(anyhow::Error::new)?;
    observer.note_config_loaded(load_result.created_default);
    if load_result.created_default {
        eprintln!(
            "imagod created default config at {}; review tls.server_key and tls.client_public_keys",
            config_path.display()
        );
    }

    let config = Arc::new(load_result.config);
    let artifact_root = config.storage_root.join("artifacts");
    let artifacts = ArtifactStore::new(
        &artifact_root,
        config.runtime.upload_session_ttl_secs,
        config.runtime.committed_session_ttl_secs,
        config.runtime.max_committed_sessions,
        config.runtime.chunk_size,
        config.runtime.max_inflight_chunks,
        config.runtime.max_artifact_size_bytes,
    )
    .await
    .map_err(anyhow::Error::new)?;
    let operations = OperationManager::new();
    let supervisor = ServiceSupervisor::new_with_config_path(
        &config.storage_root,
        config.runtime.stop_grace_timeout_secs,
        config.runtime.runner_ready_timeout_secs,
        config.runtime.manager_control_read_timeout_ms,
        config.runtime.http_worker_count,
        config.runtime.http_worker_queue_capacity,
        config.runtime.runner_log_buffer_bytes,
        config.runtime.retained_logs_capacity_bytes,
        config.runtime.epoch_tick_interval_ms,
        &config_path,
    )
    .map_err(anyhow::Error::new)?
    .with_wasm_engine_tuning(
        config.runtime.wasm_memory_reservation_bytes,
        config.runtime.wasm_memory_reservation_for_growth_bytes,
        config.runtime.wasm_memory_guard_size_bytes,
        config.runtime.wasm_guard_before_linear_memory,
        config.runtime.wasm_parallel_compilation,
    )
    .with_http_queue_memory_budget_bytes(config.runtime.http_queue_memory_budget_bytes);
    let orchestrator = Orchestrator::new(&config.storage_root, artifacts.clone(), supervisor);
    let server = build_server(config.as_ref()).map_err(anyhow::Error::new)?;

    run_boot_tasks(&orchestrator, config.as_ref(), observer).await;

    let handler = ProtocolHandler::new(
        config.clone(),
        config_path,
        artifacts,
        operations,
        orchestrator,
    );
    let maintenance = spawn_maintenance_loop(
        handler.clone(),
        config.runtime.epoch_tick_interval_ms,
        observer.clone(),
    );

    eprintln!("imagod listening on {}", config.listen_addr);
    observer.note_listening();

    Ok(BootContext {
        handler,
        server,
        maintenance,
        session_concurrency_limit: config.runtime.max_concurrent_sessions as usize,
    })
}

async fn run_boot_tasks<O>(orchestrator: &Orchestrator, config: &ImagodConfig, observer: &O)
where
    O: ManagerRuntimeObserver,
{
    if config.runtime.boot_plugin_gc_enabled {
        match orchestrator.gc_unused_plugin_components_on_boot().await {
            Ok(()) => {
                observer.note_plugin_gc(ManagerRuntimeTaskState::Succeeded);
                eprintln!("plugin component cache gc completed");
            }
            Err(err) => {
                observer.note_plugin_gc(ManagerRuntimeTaskState::Failed);
                eprintln!(
                    "plugin component cache gc failed code={:?} stage={} message={}",
                    err.code, err.stage, err.message
                );
            }
        }
    } else {
        eprintln!("plugin component cache gc skipped by runtime.boot_plugin_gc_enabled=false");
    }

    if config.runtime.boot_restore_enabled {
        match orchestrator.restore_active_services_on_boot().await {
            Ok(summary) => {
                observer.note_boot_restore(ManagerRuntimeTaskState::Succeeded);
                for started in &summary.started {
                    eprintln!(
                        "boot restore started name={} release={}",
                        started.service_name, started.release_hash
                    );
                }
                for failed in &summary.failed {
                    eprintln!(
                        "boot restore failed name={} code={:?} stage={} message={}",
                        failed.service_name,
                        failed.error.code,
                        failed.error.stage,
                        failed.error.message
                    );
                }
                eprintln!(
                    "boot restore summary started={} failed={}",
                    summary.started.len(),
                    summary.failed.len()
                );
            }
            Err(err) => {
                observer.note_boot_restore(ManagerRuntimeTaskState::Failed);
                eprintln!(
                    "boot restore scan failed code={:?} stage={} message={}",
                    err.code, err.stage, err.message
                );
            }
        }
    } else {
        eprintln!("boot restore skipped by runtime.boot_restore_enabled=false");
    }
}

fn spawn_maintenance_loop<O>(
    handler: ProtocolHandler,
    epoch_tick_interval_ms: u64,
    observer: O,
) -> MaintenanceLoop
where
    O: ManagerRuntimeObserver + Send + Sync + 'static,
{
    let active_tick_interval = Duration::from_millis(epoch_tick_interval_ms.max(1));
    let idle_tick_interval = Duration::from_secs(IDLE_MAINTENANCE_TICK_SECS);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let reap_handler = handler.clone();
    let live_handler = handler;
    let task = tokio::spawn(maintenance_loop(
        shutdown_rx,
        active_tick_interval,
        idle_tick_interval,
        observer,
        move || {
            let handler = reap_handler.clone();
            async move {
                handler.reap_finished_services().await;
            }
        },
        move || {
            let handler = live_handler.clone();
            async move { handler.has_live_services().await }
        },
    ));

    MaintenanceLoop { shutdown_tx, task }
}

async fn maintenance_loop<O, Reap, ReapFuture, HasLive, HasLiveFuture>(
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    active_tick_interval: Duration,
    idle_tick_interval: Duration,
    observer: O,
    reap_finished_services: Reap,
    has_live_services: HasLive,
) where
    O: ManagerRuntimeObserver,
    Reap: Fn() -> ReapFuture,
    ReapFuture: Future<Output = ()>,
    HasLive: Fn() -> HasLiveFuture,
    HasLiveFuture: Future<Output = bool>,
{
    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        let mut reap_future = std::pin::pin!(reap_finished_services());
        tokio::select! {
            _ = &mut reap_future => {}
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        }

        let mut has_live_services_future = std::pin::pin!(has_live_services());
        let has_live_services = tokio::select! {
            live = &mut has_live_services_future => live,
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        };

        let sleep_duration = if has_live_services {
            active_tick_interval
        } else {
            idle_tick_interval
        };
        tokio::select! {
            _ = tokio::time::sleep(sleep_duration) => {}
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    observer.note_maintenance_stopped();
}

async fn run_accept_loop<O>(
    server: &mut web_transport_quinn::Server,
    handler: ProtocolHandler,
    session_concurrency_limit: usize,
    observer: &O,
) -> AcceptLoopExit
where
    O: ManagerRuntimeObserver,
{
    let mut shutdown_signal = std::pin::pin!(tokio::signal::ctrl_c());
    let mut session_tasks = tokio::task::JoinSet::new();
    let session_concurrency = Arc::new(tokio::sync::Semaphore::new(session_concurrency_limit));
    let mut shutdown_started = false;

    loop {
        tokio::select! {
            _ = &mut shutdown_signal => {
                eprintln!("shutdown signal received");
                begin_shutdown(&handler, observer);
                shutdown_started = true;
                break;
            }
            joined = session_tasks.join_next(), if !session_tasks.is_empty() => {
                if let Some(joined) = joined {
                    log_session_task_join_result(joined);
                }
            }
            request = server.accept() => {
                let Some(request): Option<web_transport_quinn::Request> = request else {
                    break;
                };
                let permit = match session_concurrency.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        let _ = request.respond(StatusCode::TOO_MANY_REQUESTS).await;
                        continue;
                    }
                };
                let handler = handler.clone();
                session_tasks.spawn(async move {
                    let _permit = permit;
                    let Ok(session) = request.respond(StatusCode::OK).await else {
                        return;
                    };
                    if let Err(err) = handler.handle_session(session).await {
                        eprintln!("session error: {err}");
                    }
                });
            }
        }
    }

    AcceptLoopExit {
        session_tasks,
        shutdown_started,
    }
}

fn begin_shutdown<O>(handler: &ProtocolHandler, observer: &O)
where
    O: ManagerRuntimeObserver,
{
    handler.begin_shutdown();
    observer.note_shutdown_started();
}

async fn shutdown_manager<O>(
    handler: ProtocolHandler,
    maintenance: MaintenanceLoop,
    mut accept_exit: AcceptLoopExit,
    observer: &O,
) -> Result<(), anyhow::Error>
where
    O: ManagerRuntimeObserver,
{
    if !accept_exit.shutdown_started {
        begin_shutdown(&handler, observer);
    }

    drain_session_tasks(
        &mut accept_exit.session_tasks,
        Duration::from_secs(SESSION_TASK_DRAIN_TIMEOUT_SECS),
    )
    .await;

    let stop_handler = handler.clone();
    let live_handler = handler.clone();
    stop_managed_services(
        move |force| {
            let handler = stop_handler.clone();
            async move { handler.stop_all_services(force).await }
        },
        move || {
            let handler = live_handler.clone();
            async move { handler.has_live_services().await }
        },
        observer,
    )
    .await;

    let _ = maintenance.shutdown_tx.send(true);
    wait_for_maintenance_shutdown(maintenance.task).await?;
    observer.note_shutdown_completed();
    Ok(())
}

async fn stop_managed_services<O, StopAll, StopAllFuture, HasLive, HasLiveFuture>(
    stop_all_services: StopAll,
    has_live_services: HasLive,
    observer: &O,
) where
    O: ManagerRuntimeObserver,
    StopAll: Fn(bool) -> StopAllFuture,
    StopAllFuture: Future<Output = Vec<(String, ImagodError)>>,
    HasLive: Fn() -> HasLiveFuture,
    HasLiveFuture: Future<Output = bool>,
{
    log_service_shutdown_errors(stop_all_services(false).await, false);
    let forced = if has_live_services().await {
        observer.note_forced_stop_used();
        log_service_shutdown_errors(stop_all_services(true).await, true);
        true
    } else {
        false
    };
    observer.note_services_stopped(forced);
}

fn log_service_shutdown_errors(stop_errors: Vec<(String, ImagodError)>, force: bool) {
    for (service_name, err) in stop_errors {
        let prefix = if force {
            "service force-shutdown failed"
        } else {
            "service shutdown failed"
        };
        eprintln!(
            "{prefix} name={} code={:?} stage={} message={}",
            service_name, err.code, err.stage, err.message
        );
    }
}

#[cfg(test)]
mod tests;
