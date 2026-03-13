use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

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
            manager_phase: ManagerRuntimePhase::Stopped,
            listening: false,
            session_shutdown_requested: true,
            shutdown: ManagerRuntimeShutdownState {
                phase: ManagerRuntimeShutdownPhase::Completed,
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
                ManagerRuntimeTaskKind::PluginGc,
                ManagerRuntimeTaskState::Succeeded,
            ),
            ManagerRuntimeEffect::TaskMilestone(
                ManagerRuntimeTaskKind::BootRestore,
                ManagerRuntimeTaskState::Failed,
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
