use super::*;
use imagod_spec::{
    ContractEffectSummary, ManagerRuntimeOutputSummary, ManagerRuntimeProbeOutput,
    ManagerRuntimeProbeState, ShutdownStateSummary, SummaryShutdownPhase, SummaryTaskKind,
    SummaryTaskState,
};
use imagod_spec_formal::{ManagerRuntimeProjectionAction, ManagerRuntimeProjectionSpec};
use nirvash_macros::nirvash_runtime_contract;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

/// Captures externally interesting manager-runtime milestones for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct RuntimeMilestoneSnapshot {
    config_loaded: bool,
    created_default: bool,
    plugin_gc: ManagerRuntimeTaskState,
    boot_restore: ManagerRuntimeTaskState,
    listening: bool,
    manager_shutdown_started: bool,
    manager_stopped: bool,
    shutdown: ShutdownStateSummary,
}

/// Thread-safe recorder for boot, maintenance, and shutdown milestones.
#[derive(Debug, Clone, Default)]
struct RuntimeMilestoneProbe {
    inner: Arc<Mutex<RuntimeMilestoneSnapshot>>,
}

impl RuntimeMilestoneProbe {
    fn snapshot(&self) -> RuntimeMilestoneSnapshot {
        *self
            .inner
            .lock()
            .expect("manager runtime probe should not be poisoned")
    }

    fn update(&self, apply: impl FnOnce(&mut RuntimeMilestoneSnapshot)) {
        let mut guard = self
            .inner
            .lock()
            .expect("manager runtime probe should not be poisoned");
        apply(&mut guard);
    }

    fn complete_service_shutdown(&self, forced: bool) {
        self.update(|snapshot| {
            snapshot.shutdown.phase = SummaryShutdownPhase::StoppingMaintenance;
            snapshot.shutdown.sessions_drained = true;
            snapshot.shutdown.services_stopped = true;
            snapshot.shutdown.forced_stop_attempted |= forced;
        });
    }

    fn finish_shutdown(&self) {
        self.update(|snapshot| {
            snapshot.manager_stopped = true;
            snapshot.listening = false;
            snapshot.shutdown.phase = SummaryShutdownPhase::Completed;
        });
    }
}

impl ManagerRuntimeObserver for RuntimeMilestoneProbe {
    fn note_config_loaded(&self, created_default: bool) {
        self.update(|snapshot| {
            snapshot.config_loaded = true;
            snapshot.created_default = created_default;
        });
    }

    fn note_plugin_gc(&self, state: ManagerRuntimeTaskState) {
        self.update(|snapshot| {
            snapshot.plugin_gc = state;
        });
    }

    fn note_boot_restore(&self, state: ManagerRuntimeTaskState) {
        self.update(|snapshot| {
            snapshot.boot_restore = state;
        });
    }

    fn note_listening(&self) {
        self.update(|snapshot| {
            snapshot.listening = true;
        });
    }

    fn note_shutdown_started(&self) {
        self.update(|snapshot| {
            snapshot.listening = false;
            snapshot.manager_shutdown_started = true;
            snapshot.shutdown.phase = SummaryShutdownPhase::DrainingSessions;
            snapshot.shutdown.accepts_stopped = true;
        });
    }

    fn note_forced_stop_used(&self) {
        self.update(|snapshot| {
            snapshot.shutdown.forced_stop_attempted = true;
        });
    }

    fn note_maintenance_stopped(&self) {
        self.update(|snapshot| {
            snapshot.shutdown.maintenance_stopped = true;
        });
    }
}

#[derive(Debug)]
struct ManagerRuntimeProjectionRuntime {
    probe: RuntimeMilestoneProbe,
}

impl ManagerRuntimeProjectionRuntime {
    fn new() -> Self {
        Self {
            probe: RuntimeMilestoneProbe::default(),
        }
    }
}

async fn observe_manager_runtime_probe_state(
    runtime: &ManagerRuntimeProjectionRuntime,
    _context: &(),
) -> ManagerRuntimeProbeState {
    let snapshot = runtime.probe.snapshot();
    ManagerRuntimeProbeState {
        config_loaded: snapshot.config_loaded,
        created_default: snapshot.created_default,
        plugin_gc: match snapshot.plugin_gc {
            ManagerRuntimeTaskState::NotStarted => SummaryTaskState::NotStarted,
            ManagerRuntimeTaskState::Succeeded => SummaryTaskState::Succeeded,
            ManagerRuntimeTaskState::Failed => SummaryTaskState::Failed,
        },
        boot_restore: match snapshot.boot_restore {
            ManagerRuntimeTaskState::NotStarted => SummaryTaskState::NotStarted,
            ManagerRuntimeTaskState::Succeeded => SummaryTaskState::Succeeded,
            ManagerRuntimeTaskState::Failed => SummaryTaskState::Failed,
        },
        listening: snapshot.listening,
        manager_shutdown_started: snapshot.manager_shutdown_started,
        manager_stopped: snapshot.manager_stopped,
        session_shutdown_requested: snapshot.manager_shutdown_started,
        shutdown: snapshot.shutdown,
    }
}

fn observe_manager_runtime_probe_output(
    _runtime: &ManagerRuntimeProjectionRuntime,
    _context: &(),
    action: &ManagerRuntimeProjectionAction,
    _result: &(),
) -> ManagerRuntimeProbeOutput {
    let effects = match action {
        ManagerRuntimeProjectionAction::RunPluginGcSucceeded => {
            vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::PluginGc,
                SummaryTaskState::Succeeded,
            )]
        }
        ManagerRuntimeProjectionAction::RunPluginGcFailed => {
            vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::PluginGc,
                SummaryTaskState::Failed,
            )]
        }
        ManagerRuntimeProjectionAction::RunBootRestoreSucceeded => {
            vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::BootRestore,
                SummaryTaskState::Succeeded,
            )]
        }
        ManagerRuntimeProjectionAction::RunBootRestoreFailed => {
            vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::BootRestore,
                SummaryTaskState::Failed,
            )]
        }
        ManagerRuntimeProjectionAction::FinishShutdown => {
            vec![ContractEffectSummary::ShutdownComplete]
        }
        _ => Vec::new(),
    };
    ManagerRuntimeProbeOutput {
        output: ManagerRuntimeOutputSummary { effects },
    }
}

#[nirvash_runtime_contract(
    spec = ManagerRuntimeProjectionSpec,
    binding = ManagerRuntimeProjectionBinding,
    context = (),
    context_expr = (),
    probe_state = ManagerRuntimeProbeState,
    probe_output = ManagerRuntimeProbeOutput,
    observe_state = observe_manager_runtime_probe_state,
    output = observe_manager_runtime_probe_output,
    fresh_runtime = ManagerRuntimeProjectionRuntime::new(),
    tests(grouped)
)]
impl ManagerRuntimeProjectionRuntime {
    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::LoadExistingConfig)]
    async fn contract_load_existing_config(&self) {
        self.probe.note_config_loaded(false);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::CreateDefaultConfig)]
    async fn contract_create_default_config(&self) {
        self.probe.note_config_loaded(true);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::RunPluginGcSucceeded)]
    async fn contract_run_plugin_gc_succeeded(&self) {
        self.probe
            .note_plugin_gc(ManagerRuntimeTaskState::Succeeded);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::RunPluginGcFailed)]
    async fn contract_run_plugin_gc_failed(&self) {
        self.probe.note_plugin_gc(ManagerRuntimeTaskState::Failed);
    }

    #[nirvash_macros::contract_case(
        action = ManagerRuntimeProjectionAction::RunBootRestoreSucceeded
    )]
    async fn contract_run_boot_restore_succeeded(&self) {
        self.probe
            .note_boot_restore(ManagerRuntimeTaskState::Succeeded);
        self.probe.note_listening();
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::RunBootRestoreFailed)]
    async fn contract_run_boot_restore_failed(&self) {
        self.probe
            .note_boot_restore(ManagerRuntimeTaskState::Failed);
        self.probe.note_listening();
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::BeginShutdown)]
    async fn contract_begin_shutdown(&self) {
        self.probe.note_shutdown_started();
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::StopServicesGraceful)]
    async fn contract_stop_services_graceful(&self) {
        stop_managed_services(|_| async { Vec::new() }, || async { false }, &self.probe).await;
        self.probe.complete_service_shutdown(false);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::StopServicesForced)]
    async fn contract_stop_services_forced(&self) {
        stop_managed_services(|_| async { Vec::new() }, || async { true }, &self.probe).await;
        self.probe.complete_service_shutdown(true);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::StopMaintenance)]
    async fn contract_stop_maintenance(&self) {
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

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::FinishShutdown)]
    async fn contract_finish_shutdown(&self) {
        self.probe.finish_shutdown();
    }
}

#[test]
fn manager_runtime_probe_records_boot_and_shutdown_milestones() {
    let probe = RuntimeMilestoneProbe::default();

    probe.note_config_loaded(true);
    probe.note_plugin_gc(ManagerRuntimeTaskState::Succeeded);
    probe.note_boot_restore(ManagerRuntimeTaskState::Failed);
    probe.note_listening();
    probe.note_shutdown_started();
    probe.complete_service_shutdown(true);
    probe.note_forced_stop_used();
    probe.note_maintenance_stopped();
    probe.finish_shutdown();

    assert_eq!(
        probe.snapshot(),
        RuntimeMilestoneSnapshot {
            config_loaded: true,
            created_default: true,
            plugin_gc: ManagerRuntimeTaskState::Succeeded,
            boot_restore: ManagerRuntimeTaskState::Failed,
            listening: false,
            manager_shutdown_started: true,
            manager_stopped: true,
            shutdown: ShutdownStateSummary {
                phase: SummaryShutdownPhase::Completed,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: true,
                forced_stop_attempted: true,
            },
        }
    );
}

#[tokio::test]
async fn maintenance_loop_marks_probe_stopped_after_shutdown_signal() {
    let probe = RuntimeMilestoneProbe::default();
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
        probe.snapshot().shutdown.maintenance_stopped,
        "maintenance probe should record stop"
    );
    assert!(
        reap_count.load(Ordering::SeqCst) > 0,
        "maintenance loop should attempt at least one reap"
    );
}

#[tokio::test]
async fn stop_managed_services_records_forced_stop_when_services_remain() {
    let probe = RuntimeMilestoneProbe::default();
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
        probe.snapshot().shutdown.forced_stop_attempted,
        "forced stop should be recorded when services stay alive"
    );
}

#[tokio::test]
async fn stop_managed_services_skips_forced_stop_when_services_are_gone() {
    let probe = RuntimeMilestoneProbe::default();
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
        !probe.snapshot().shutdown.forced_stop_attempted,
        "forced stop should stay false when the first drain succeeds"
    );
}
