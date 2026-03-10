use super::*;
use imagod_spec::{
    ContractEffectSummary, ManagerRuntimeOutputSummary, ManagerRuntimeProbeOutput,
    ManagerRuntimeProbeState, SummaryManagerRuntimePhase, SummaryShutdownPhase, SummaryTaskKind,
    SummaryTaskState,
};
use imagod_spec_formal::{ManagerRuntimeProjectionAction, ManagerRuntimeProjectionSpec};
use nirvash_macros::nirvash_runtime_contract;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn observe_manager_runtime_probe_state(
    runtime: &ManagerRuntimeProjectionRuntime,
    _context: &(),
) -> ManagerRuntimeProbeState {
    let snapshot = runtime.observation.snapshot();
    ManagerRuntimeProbeState {
        config_loaded: snapshot.config_loaded,
        created_default: snapshot.created_default,
        manager_phase: snapshot.manager_phase,
        listening: snapshot.listening,
        manager_shutdown_started: matches!(
            snapshot.manager_phase,
            SummaryManagerRuntimePhase::ShutdownRequested | SummaryManagerRuntimePhase::Stopped
        ),
        manager_stopped: matches!(snapshot.manager_phase, SummaryManagerRuntimePhase::Stopped),
        session_shutdown_requested: snapshot.session_shutdown_requested,
        shutdown: snapshot.shutdown,
    }
}

fn observe_manager_runtime_probe_output(
    runtime: &ManagerRuntimeProjectionRuntime,
    _context: &(),
    _action: &ManagerRuntimeProjectionAction,
    _result: &(),
) -> ManagerRuntimeProbeOutput {
    let effects = runtime
        .observation
        .drain_effects()
        .into_iter()
        .map(|effect| match effect {
            ManagerRuntimeEffect::TaskMilestone(kind, state) => {
                ContractEffectSummary::TaskMilestone(kind, state)
            }
            ManagerRuntimeEffect::ShutdownComplete => ContractEffectSummary::ShutdownComplete,
        })
        .collect();
    ManagerRuntimeProbeOutput {
        output: ManagerRuntimeOutputSummary { effects },
    }
}

#[derive(Debug)]
struct ManagerRuntimeProjectionRuntime {
    observation: ManagerRuntimeObservation,
}

impl ManagerRuntimeProjectionRuntime {
    fn new() -> Self {
        Self {
            observation: ManagerRuntimeObservation::default(),
        }
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
        self.observation.note_config_loaded(false);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::CreateDefaultConfig)]
    async fn contract_create_default_config(&self) {
        self.observation.note_config_loaded(true);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::RunPluginGcSucceeded)]
    async fn contract_run_plugin_gc_succeeded(&self) {
        self.observation
            .note_plugin_gc(ManagerRuntimeTaskState::Succeeded);
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::RunPluginGcFailed)]
    async fn contract_run_plugin_gc_failed(&self) {
        self.observation
            .note_plugin_gc(ManagerRuntimeTaskState::Failed);
    }

    #[nirvash_macros::contract_case(
        action = ManagerRuntimeProjectionAction::RunBootRestoreSucceeded
    )]
    async fn contract_run_boot_restore_succeeded(&self) {
        self.observation
            .note_boot_restore(ManagerRuntimeTaskState::Succeeded);
        self.observation.note_listening();
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::RunBootRestoreFailed)]
    async fn contract_run_boot_restore_failed(&self) {
        self.observation
            .note_boot_restore(ManagerRuntimeTaskState::Failed);
        self.observation.note_listening();
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::BeginShutdown)]
    async fn contract_begin_shutdown(&self) {
        self.observation.note_shutdown_started();
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::StopServicesGraceful)]
    async fn contract_stop_services_graceful(&self) {
        stop_managed_services(
            |_| async { Vec::new() },
            || async { false },
            &self.observation,
        )
        .await;
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::StopServicesForced)]
    async fn contract_stop_services_forced(&self) {
        stop_managed_services(
            |_| async { Vec::new() },
            || async { true },
            &self.observation,
        )
        .await;
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::StopMaintenance)]
    async fn contract_stop_maintenance(&self) {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let observation = self.observation.clone();
        let task = tokio::spawn(maintenance_loop(
            shutdown_rx,
            Duration::from_millis(1),
            Duration::from_millis(1),
            observation,
            || async {},
            || async { false },
        ));
        let _ = shutdown_tx.send(true);
        task.await.expect("maintenance loop should join");
    }

    #[nirvash_macros::contract_case(action = ManagerRuntimeProjectionAction::FinishShutdown)]
    async fn contract_finish_shutdown(&self) {
        self.observation.note_shutdown_completed();
    }
}

#[test]
fn manager_runtime_observation_records_snapshot_and_effects() {
    let observation = ManagerRuntimeObservation::default();

    observation.note_config_loaded(true);
    observation.note_plugin_gc(ManagerRuntimeTaskState::Succeeded);
    observation.note_boot_restore(ManagerRuntimeTaskState::Failed);
    observation.note_listening();
    observation.note_shutdown_started();
    observation.note_services_stopped(true);
    observation.note_forced_stop_used();
    observation.note_maintenance_stopped();
    observation.note_shutdown_completed();

    assert_eq!(
        observation.snapshot(),
        ManagerRuntimeSnapshot {
            config_loaded: true,
            created_default: true,
            manager_phase: SummaryManagerRuntimePhase::Stopped,
            listening: false,
            session_shutdown_requested: true,
            shutdown: imagod_spec::ShutdownStateSummary {
                phase: SummaryShutdownPhase::Completed,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: true,
                forced_stop_attempted: true,
            },
        }
    );
    assert_eq!(
        observation.drain_effects(),
        vec![
            ManagerRuntimeEffect::TaskMilestone(
                SummaryTaskKind::PluginGc,
                SummaryTaskState::Succeeded,
            ),
            ManagerRuntimeEffect::TaskMilestone(
                SummaryTaskKind::BootRestore,
                SummaryTaskState::Failed,
            ),
            ManagerRuntimeEffect::ShutdownComplete,
        ]
    );
}

#[tokio::test]
async fn maintenance_loop_marks_snapshot_stopped_after_shutdown_signal() {
    let observation = ManagerRuntimeObservation::default();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let reap_count = Arc::new(AtomicUsize::new(0));

    let task = tokio::spawn(maintenance_loop(
        shutdown_rx,
        Duration::from_millis(1),
        Duration::from_millis(1),
        observation.clone(),
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
        observation.snapshot().shutdown.maintenance_stopped,
        "maintenance snapshot should record stop"
    );
    assert!(
        reap_count.load(Ordering::SeqCst) > 0,
        "maintenance loop should attempt at least one reap"
    );
}

#[tokio::test]
async fn stop_managed_services_records_forced_stop_when_services_remain() {
    let observation = ManagerRuntimeObservation::default();
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
        &observation,
    )
    .await;

    assert_eq!(stop_count.load(Ordering::SeqCst), 2);
    assert_eq!(live_checks.load(Ordering::SeqCst), 1);
    assert!(
        observation.snapshot().shutdown.forced_stop_attempted,
        "forced stop should be recorded when services stay alive"
    );
    assert!(
        observation.snapshot().shutdown.services_stopped,
        "service shutdown completion should be recorded"
    );
}

#[tokio::test]
async fn stop_managed_services_skips_forced_stop_when_services_are_gone() {
    let observation = ManagerRuntimeObservation::default();
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
        &observation,
    )
    .await;

    assert_eq!(stop_count.load(Ordering::SeqCst), 1);
    assert!(
        !observation.snapshot().shutdown.forced_stop_attempted,
        "forced stop should stay false when the first drain succeeds"
    );
    assert!(
        observation.snapshot().shutdown.services_stopped,
        "graceful shutdown should still mark services stopped"
    );
}
