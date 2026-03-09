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

/// Captures externally interesting manager-runtime milestones for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ManagerRuntimeRecord {
    pub config_loaded: bool,
    pub created_default: bool,
    pub plugin_gc: ManagerRuntimeTaskState,
    pub boot_restore: ManagerRuntimeTaskState,
    pub listening: bool,
    pub shutdown_started: bool,
    pub forced_stop_used: bool,
    pub maintenance_stopped: bool,
}

/// Thread-safe recorder for boot, maintenance, and shutdown milestones.
#[derive(Debug, Clone, Default)]
pub(crate) struct ManagerRuntimeProbe {
    inner: Arc<Mutex<ManagerRuntimeRecord>>,
}

impl ManagerRuntimeProbe {
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> ManagerRuntimeRecord {
        *self
            .inner
            .lock()
            .expect("manager runtime probe should not be poisoned")
    }

    fn update(&self, update: impl FnOnce(&mut ManagerRuntimeRecord)) {
        let mut guard = self
            .inner
            .lock()
            .expect("manager runtime probe should not be poisoned");
        update(&mut guard);
    }

    fn record_config_loaded(&self, created_default: bool) {
        self.update(|record| {
            record.config_loaded = true;
            record.created_default = created_default;
        });
    }

    fn record_plugin_gc(&self, state: ManagerRuntimeTaskState) {
        self.update(|record| {
            record.plugin_gc = state;
        });
    }

    fn record_boot_restore(&self, state: ManagerRuntimeTaskState) {
        self.update(|record| {
            record.boot_restore = state;
        });
    }

    fn record_listening(&self) {
        self.update(|record| {
            record.listening = true;
        });
    }

    fn record_shutdown_started(&self) {
        self.update(|record| {
            record.shutdown_started = true;
        });
    }

    fn record_forced_stop_used(&self) {
        self.update(|record| {
            record.forced_stop_used = true;
        });
    }

    fn record_maintenance_stopped(&self) {
        self.update(|record| {
            record.maintenance_stopped = true;
        });
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
    run_manager_with_probe(config_path, ManagerRuntimeProbe::default()).await
}

async fn run_manager_with_probe(
    config_path: Option<PathBuf>,
    probe: ManagerRuntimeProbe,
) -> Result<(), anyhow::Error> {
    let mut boot = boot_manager(config_path, &probe).await?;
    let accept_exit = run_accept_loop(
        &mut boot.server,
        boot.handler.clone(),
        boot.session_concurrency_limit,
        &probe,
    )
    .await;
    shutdown_manager(boot.handler, boot.maintenance, accept_exit, &probe).await
}

async fn boot_manager(
    config_path: Option<PathBuf>,
    probe: &ManagerRuntimeProbe,
) -> Result<BootContext, anyhow::Error> {
    let config_path = resolve_config_path(config_path);
    let load_result = load_or_create_default(&config_path).map_err(anyhow::Error::new)?;
    probe.record_config_loaded(load_result.created_default);
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

    run_boot_tasks(&orchestrator, config.as_ref(), probe).await;

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
        probe.clone(),
    );

    eprintln!("imagod listening on {}", config.listen_addr);
    probe.record_listening();

    Ok(BootContext {
        handler,
        server,
        maintenance,
        session_concurrency_limit: config.runtime.max_concurrent_sessions as usize,
    })
}

async fn run_boot_tasks(
    orchestrator: &Orchestrator,
    config: &ImagodConfig,
    probe: &ManagerRuntimeProbe,
) {
    if config.runtime.boot_plugin_gc_enabled {
        match orchestrator.gc_unused_plugin_components_on_boot().await {
            Ok(()) => {
                probe.record_plugin_gc(ManagerRuntimeTaskState::Succeeded);
                eprintln!("plugin component cache gc completed");
            }
            Err(err) => {
                probe.record_plugin_gc(ManagerRuntimeTaskState::Failed);
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
                probe.record_boot_restore(ManagerRuntimeTaskState::Succeeded);
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
                probe.record_boot_restore(ManagerRuntimeTaskState::Failed);
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

fn spawn_maintenance_loop(
    handler: ProtocolHandler,
    epoch_tick_interval_ms: u64,
    probe: ManagerRuntimeProbe,
) -> MaintenanceLoop {
    let active_tick_interval = Duration::from_millis(epoch_tick_interval_ms.max(1));
    let idle_tick_interval = Duration::from_secs(IDLE_MAINTENANCE_TICK_SECS);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let reap_handler = handler.clone();
    let live_handler = handler;
    let task = tokio::spawn(maintenance_loop(
        shutdown_rx,
        active_tick_interval,
        idle_tick_interval,
        probe,
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

async fn maintenance_loop<Reap, ReapFuture, HasLive, HasLiveFuture>(
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    active_tick_interval: Duration,
    idle_tick_interval: Duration,
    probe: ManagerRuntimeProbe,
    reap_finished_services: Reap,
    has_live_services: HasLive,
) where
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

    probe.record_maintenance_stopped();
}

async fn run_accept_loop(
    server: &mut web_transport_quinn::Server,
    handler: ProtocolHandler,
    session_concurrency_limit: usize,
    probe: &ManagerRuntimeProbe,
) -> AcceptLoopExit {
    let mut shutdown_signal = std::pin::pin!(tokio::signal::ctrl_c());
    let mut session_tasks = tokio::task::JoinSet::new();
    let session_concurrency = Arc::new(tokio::sync::Semaphore::new(session_concurrency_limit));
    let mut shutdown_started = false;

    loop {
        tokio::select! {
            _ = &mut shutdown_signal => {
                eprintln!("shutdown signal received");
                begin_shutdown(&handler, probe);
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

fn begin_shutdown(handler: &ProtocolHandler, probe: &ManagerRuntimeProbe) {
    handler.begin_shutdown();
    probe.record_shutdown_started();
}

async fn shutdown_manager(
    handler: ProtocolHandler,
    maintenance: MaintenanceLoop,
    mut accept_exit: AcceptLoopExit,
    probe: &ManagerRuntimeProbe,
) -> Result<(), anyhow::Error> {
    if !accept_exit.shutdown_started {
        begin_shutdown(&handler, probe);
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
        probe,
    )
    .await;

    let _ = maintenance.shutdown_tx.send(true);
    wait_for_maintenance_shutdown(maintenance.task).await?;
    Ok(())
}

async fn stop_managed_services<StopAll, StopAllFuture, HasLive, HasLiveFuture>(
    stop_all_services: StopAll,
    has_live_services: HasLive,
    probe: &ManagerRuntimeProbe,
) where
    StopAll: Fn(bool) -> StopAllFuture,
    StopAllFuture: Future<Output = Vec<(String, ImagodError)>>,
    HasLive: Fn() -> HasLiveFuture,
    HasLiveFuture: Future<Output = bool>,
{
    log_service_shutdown_errors(stop_all_services(false).await, false);
    if has_live_services().await {
        probe.record_forced_stop_used();
        log_service_shutdown_errors(stop_all_services(true).await, true);
    }
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use imagod_spec::{
        ManagerRuntimeProjectionAction, ManagerRuntimeProjectionObservedState,
        ManagerRuntimeProjectionSpec, SystemEffect,
    };
    use nirvash_core::{
        TransitionSystem,
        conformance::{ActionApplier, ProtocolRuntimeBinding, StateObserver},
    };
    use nirvash_macros::code_tests;
    use tokio::sync::Mutex as AsyncMutex;

    #[derive(Debug)]
    struct ManagerRuntimeProjectionRuntime {
        probe: ManagerRuntimeProbe,
        state: AsyncMutex<imagod_spec::SystemState>,
        trace: AsyncMutex<Vec<ManagerRuntimeProjectionAction>>,
    }

    impl ManagerRuntimeProjectionRuntime {
        fn new() -> Self {
            Self {
                probe: ManagerRuntimeProbe::default(),
                state: AsyncMutex::new(ManagerRuntimeProjectionSpec::new().initial_state()),
                trace: AsyncMutex::new(Vec::new()),
            }
        }

        async fn push_trace(&self, action: ManagerRuntimeProjectionAction) {
            self.trace.lock().await.push(action);
        }
    }

    impl ActionApplier for ManagerRuntimeProjectionRuntime {
        type Action = ManagerRuntimeProjectionAction;
        type Output = Vec<SystemEffect>;
        type Context = ();

        async fn execute_action(
            &self,
            _context: &Self::Context,
            action: &Self::Action,
        ) -> Self::Output {
            let spec = ManagerRuntimeProjectionSpec::new();
            let mut state = self.state.lock().await;
            let Some(next) = spec.transition(&state, action) else {
                return Vec::new();
            };
            match action {
                ManagerRuntimeProjectionAction::LoadExistingConfig => {
                    self.probe.record_config_loaded(false);
                }
                ManagerRuntimeProjectionAction::CreateDefaultConfig => {
                    self.probe.record_config_loaded(true);
                }
                ManagerRuntimeProjectionAction::RunPluginGcSucceeded => {
                    self.probe
                        .record_plugin_gc(ManagerRuntimeTaskState::Succeeded);
                }
                ManagerRuntimeProjectionAction::RunPluginGcFailed => {
                    self.probe.record_plugin_gc(ManagerRuntimeTaskState::Failed);
                }
                ManagerRuntimeProjectionAction::RunBootRestoreSucceeded => {
                    self.probe
                        .record_boot_restore(ManagerRuntimeTaskState::Succeeded);
                    self.probe.record_listening();
                }
                ManagerRuntimeProjectionAction::RunBootRestoreFailed => {
                    self.probe.record_boot_restore(ManagerRuntimeTaskState::Failed);
                    self.probe.record_listening();
                }
                ManagerRuntimeProjectionAction::BeginShutdown => {
                    self.probe.record_shutdown_started();
                }
                ManagerRuntimeProjectionAction::StopServicesGraceful => {
                    stop_managed_services(
                        |_| async { Vec::new() },
                        || async { false },
                        &self.probe,
                    )
                    .await;
                }
                ManagerRuntimeProjectionAction::StopServicesForced => {
                    stop_managed_services(
                        |_| async { Vec::new() },
                        || async { true },
                        &self.probe,
                    )
                    .await;
                }
                ManagerRuntimeProjectionAction::StopMaintenance => {
                    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
                    let probe = self.probe.clone();
                    let task = tokio::spawn(maintenance_loop(
                        shutdown_rx,
                        Duration::from_millis(1),
                        Duration::from_millis(1),
                        probe,
                        || async {},
                        || async { false },
                    ));
                    let _ = shutdown_tx.send(true);
                    task.await.expect("maintenance loop should join");
                }
                ManagerRuntimeProjectionAction::FinishShutdown => {}
            }
            *state = next;
            self.push_trace(*action).await;
            if matches!(action, ManagerRuntimeProjectionAction::FinishShutdown) {
                vec![SystemEffect::ShutdownComplete]
            } else {
                Vec::new()
            }
        }
    }

    impl StateObserver for ManagerRuntimeProjectionRuntime {
        type ObservedState = ManagerRuntimeProjectionObservedState;
        type Context = ();

        async fn observe_state(&self, _context: &Self::Context) -> Self::ObservedState {
            ManagerRuntimeProjectionObservedState {
                trace: self.trace.lock().await.clone(),
            }
        }
    }

    #[derive(Debug, Default, Clone, Copy)]
    struct ManagerRuntimeProjectionBinding;

    impl ProtocolRuntimeBinding<ManagerRuntimeProjectionSpec> for ManagerRuntimeProjectionBinding {
        type Runtime = ManagerRuntimeProjectionRuntime;
        type Context = ();

        async fn fresh_runtime(_spec: &ManagerRuntimeProjectionSpec) -> Self::Runtime {
            ManagerRuntimeProjectionRuntime::new()
        }

        fn context(_spec: &ManagerRuntimeProjectionSpec) -> Self::Context {}
    }

    #[code_tests(
        spec = ManagerRuntimeProjectionSpec,
        binding = ManagerRuntimeProjectionBinding
    )]
    const _: () = ();

    #[test]
    fn manager_runtime_probe_records_boot_and_shutdown_milestones() {
        let probe = ManagerRuntimeProbe::default();

        probe.record_config_loaded(true);
        probe.record_plugin_gc(ManagerRuntimeTaskState::Succeeded);
        probe.record_boot_restore(ManagerRuntimeTaskState::Failed);
        probe.record_listening();
        probe.record_shutdown_started();
        probe.record_forced_stop_used();
        probe.record_maintenance_stopped();

        assert_eq!(
            probe.snapshot(),
            ManagerRuntimeRecord {
                config_loaded: true,
                created_default: true,
                plugin_gc: ManagerRuntimeTaskState::Succeeded,
                boot_restore: ManagerRuntimeTaskState::Failed,
                listening: true,
                shutdown_started: true,
                forced_stop_used: true,
                maintenance_stopped: true,
            }
        );
    }

    #[tokio::test]
    async fn maintenance_loop_marks_probe_stopped_after_shutdown_signal() {
        let probe = ManagerRuntimeProbe::default();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let reap_count = Arc::new(AtomicUsize::new(0));

        let task = tokio::spawn(maintenance_loop(
            shutdown_rx,
            Duration::from_millis(1),
            Duration::from_millis(1),
            probe.clone(),
            {
                let reap_count = reap_count.clone();
                move || {
                    let reap_count = reap_count.clone();
                    async move {
                        reap_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            },
            || async { false },
        ));

        tokio::time::sleep(Duration::from_millis(10)).await;
        shutdown_tx
            .send(true)
            .expect("maintenance shutdown signal should send");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("maintenance loop should stop")
            .expect("maintenance loop should join cleanly");

        assert!(
            probe.snapshot().maintenance_stopped,
            "maintenance probe should record stop"
        );
        assert!(
            reap_count.load(Ordering::SeqCst) > 0,
            "maintenance loop should attempt at least one reap"
        );
    }

    #[tokio::test]
    async fn stop_managed_services_records_forced_stop_when_services_remain() {
        let probe = ManagerRuntimeProbe::default();
        let stop_count = Arc::new(AtomicUsize::new(0));
        let live_checks = Arc::new(AtomicUsize::new(0));

        stop_managed_services(
            {
                let stop_count = stop_count.clone();
                move |_| {
                    let stop_count = stop_count.clone();
                    async move {
                        stop_count.fetch_add(1, Ordering::SeqCst);
                        Vec::new()
                    }
                }
            },
            {
                let live_checks = live_checks.clone();
                move || {
                    let live_checks = live_checks.clone();
                    async move {
                        live_checks.fetch_add(1, Ordering::SeqCst);
                        true
                    }
                }
            },
            &probe,
        )
        .await;

        assert_eq!(stop_count.load(Ordering::SeqCst), 2);
        assert_eq!(live_checks.load(Ordering::SeqCst), 1);
        assert!(
            probe.snapshot().forced_stop_used,
            "forced stop should be recorded when services stay alive"
        );
    }

    #[tokio::test]
    async fn stop_managed_services_skips_forced_stop_when_services_are_gone() {
        let probe = ManagerRuntimeProbe::default();
        let stop_count = Arc::new(AtomicUsize::new(0));

        stop_managed_services(
            {
                let stop_count = stop_count.clone();
                move |_| {
                    let stop_count = stop_count.clone();
                    async move {
                        stop_count.fetch_add(1, Ordering::SeqCst);
                        Vec::new()
                    }
                }
            },
            || async { false },
            &probe,
        )
        .await;

        assert_eq!(stop_count.load(Ordering::SeqCst), 1);
        assert!(
            !probe.snapshot().forced_stop_used,
            "forced stop should stay false when the first drain succeeds"
        );
    }
}
