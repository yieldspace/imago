use imagod_spec::{
    ManagerRuntimeOutputSummary, ManagerRuntimeProbeOutput, ManagerRuntimeProbeState,
    ManagerRuntimeStateSummary,
};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    concurrent::ConcurrentAction, conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature, nirvash_projection_contract};

use crate::{
    manager_runtime::{ManagerRuntimeAction, ManagerRuntimeSpec},
    session_transport::{SessionTransportAction, SessionTransportSpec},
    shutdown_flow::{ShutdownFlowAction, ShutdownFlowSpec, ShutdownPhase},
    summary_mapping::{shutdown_phase, system_effects, task_state},
    system::{SystemAtomicAction, SystemEffect, SystemSpec, SystemState},
};

/// Boot/maintenance/shutdown milestones projected from the unified `system` spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum ManagerRuntimeProjectionAction {
    /// Observe loading an existing config.
    LoadExistingConfig,
    /// Observe creating the default config.
    CreateDefaultConfig,
    /// Observe plugin GC success during boot.
    RunPluginGcSucceeded,
    /// Observe plugin GC failure during boot.
    RunPluginGcFailed,
    /// Observe boot restore success.
    RunBootRestoreSucceeded,
    /// Observe boot restore failure.
    RunBootRestoreFailed,
    /// Observe shutdown signal and stop-accepting.
    BeginShutdown,
    /// Observe graceful service stop during shutdown.
    StopServicesGraceful,
    /// Observe forced service stop fallback.
    StopServicesForced,
    /// Observe maintenance loop stop.
    StopMaintenance,
    /// Observe final manager stop.
    FinishShutdown,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ManagerRuntimeProjectionSpec;

impl ManagerRuntimeProjectionSpec {
    pub const fn new() -> Self {
        Self
    }

    fn system(self) -> SystemSpec {
        SystemSpec::new()
    }

    pub fn initial_state(self) -> SystemState {
        self.system().boot_state()
    }

    fn apply_atomic(self, state: &SystemState, action: SystemAtomicAction) -> Option<SystemState> {
        self.system()
            .transition(state, &ConcurrentAction::from_atomic(action))
    }

    fn apply_begin_shutdown(self, state: &SystemState) -> Option<SystemState> {
        let mut candidate = state.clone();
        candidate.manager = ManagerRuntimeSpec::new()
            .transition(&state.manager, &ManagerRuntimeAction::BeginShutdown)?;
        candidate.shutdown = ShutdownFlowSpec::new()
            .transition(&state.shutdown, &ShutdownFlowAction::ReceiveSignal)?;
        candidate.session = SessionTransportSpec::new()
            .transition(&state.session, &SessionTransportAction::BeginShutdown)?;
        candidate.shutdown = ShutdownFlowSpec::new()
            .transition(&candidate.shutdown, &ShutdownFlowAction::StopAccepting)?;
        Some(candidate)
    }

    fn apply_stop_services(self, state: &SystemState, force: bool) -> Option<SystemState> {
        let mut candidate = state.clone();
        if matches!(candidate.shutdown.phase, ShutdownPhase::DrainingSessions) {
            candidate.shutdown = ShutdownFlowSpec::new()
                .transition(&candidate.shutdown, &ShutdownFlowAction::DrainSessions)?;
        }
        let action = if force {
            ShutdownFlowAction::StopServicesForced
        } else {
            ShutdownFlowAction::StopServicesGraceful
        };
        candidate.shutdown = ShutdownFlowSpec::new().transition(&candidate.shutdown, &action)?;
        Some(candidate)
    }

    fn apply_finish_shutdown(self, state: &SystemState) -> Option<SystemState> {
        let mut candidate = state.clone();
        if !matches!(candidate.shutdown.phase, ShutdownPhase::Completed) {
            candidate.shutdown = ShutdownFlowSpec::new()
                .transition(&candidate.shutdown, &ShutdownFlowAction::Finalize)?;
        }
        candidate.manager = ManagerRuntimeSpec::new()
            .transition(&candidate.manager, &ManagerRuntimeAction::FinishShutdown)?;
        Some(candidate)
    }
}

fn summarize_manager_runtime_state(probe: &ManagerRuntimeProbeState) -> ManagerRuntimeStateSummary {
    *probe
}

fn summarize_manager_runtime_output(
    probe: &ManagerRuntimeProbeOutput,
) -> ManagerRuntimeOutputSummary {
    probe.clone()
}

fn abstract_manager_runtime_state(
    spec: &ManagerRuntimeProjectionSpec,
    summary: &ManagerRuntimeStateSummary,
) -> SystemState {
    let mut state = spec.initial_state();
    state.manager.config_loaded = summary.config_loaded;
    state.manager.created_default = summary.created_default;
    state.manager.plugin_gc = task_state(summary.plugin_gc);
    state.manager.boot_restore = task_state(summary.boot_restore);
    state.manager.phase = if summary.manager_stopped {
        crate::manager_runtime::ManagerRuntimePhase::Stopped
    } else if summary.manager_shutdown_started {
        crate::manager_runtime::ManagerRuntimePhase::ShutdownRequested
    } else if summary.listening {
        crate::manager_runtime::ManagerRuntimePhase::Listening
    } else if summary.config_loaded
        && matches!(summary.plugin_gc, imagod_spec::SummaryTaskState::NotStarted)
    {
        crate::manager_runtime::ManagerRuntimePhase::ConfigReady
    } else if summary.config_loaded {
        crate::manager_runtime::ManagerRuntimePhase::Restoring
    } else {
        crate::manager_runtime::ManagerRuntimePhase::Booting
    };
    state.session.shutdown_requested = summary.session_shutdown_requested;
    state.shutdown.phase = shutdown_phase(summary.shutdown.phase);
    state.shutdown.accepts_stopped = summary.shutdown.accepts_stopped;
    state.shutdown.sessions_drained = summary.shutdown.sessions_drained;
    state.shutdown.services_stopped = summary.shutdown.services_stopped;
    state.shutdown.maintenance_stopped = summary.shutdown.maintenance_stopped;
    state.shutdown.forced_stop_attempted = summary.shutdown.forced_stop_attempted;
    state
}

fn abstract_manager_runtime_output(
    _spec: &ManagerRuntimeProjectionSpec,
    summary: &ManagerRuntimeOutputSummary,
) -> Vec<SystemEffect> {
    system_effects(&summary.effects)
}

impl TransitionSystem for ManagerRuntimeProjectionSpec {
    type State = SystemState;
    type Action = ManagerRuntimeProjectionAction;

    fn name(&self) -> &'static str {
        "manager_runtime_projection"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        let next = match action {
            ManagerRuntimeProjectionAction::LoadExistingConfig => self.apply_atomic(
                state,
                SystemAtomicAction::Manager(ManagerRuntimeAction::LoadExistingConfig),
            )?,
            ManagerRuntimeProjectionAction::CreateDefaultConfig => self.apply_atomic(
                state,
                SystemAtomicAction::Manager(ManagerRuntimeAction::CreateDefaultConfig),
            )?,
            ManagerRuntimeProjectionAction::RunPluginGcSucceeded => self.apply_atomic(
                state,
                SystemAtomicAction::Manager(ManagerRuntimeAction::RunPluginGcSucceeded),
            )?,
            ManagerRuntimeProjectionAction::RunPluginGcFailed => self.apply_atomic(
                state,
                SystemAtomicAction::Manager(ManagerRuntimeAction::RunPluginGcFailed),
            )?,
            ManagerRuntimeProjectionAction::RunBootRestoreSucceeded => self.apply_atomic(
                state,
                SystemAtomicAction::Manager(ManagerRuntimeAction::RunBootRestoreSucceeded),
            )?,
            ManagerRuntimeProjectionAction::RunBootRestoreFailed => self.apply_atomic(
                state,
                SystemAtomicAction::Manager(ManagerRuntimeAction::RunBootRestoreFailed),
            )?,
            ManagerRuntimeProjectionAction::BeginShutdown => self.apply_begin_shutdown(state)?,
            ManagerRuntimeProjectionAction::StopServicesGraceful => {
                self.apply_stop_services(state, false)?
            }
            ManagerRuntimeProjectionAction::StopServicesForced => {
                self.apply_stop_services(state, true)?
            }
            ManagerRuntimeProjectionAction::StopMaintenance => self.apply_atomic(
                state,
                SystemAtomicAction::Shutdown(ShutdownFlowAction::StopMaintenance),
            )?,
            ManagerRuntimeProjectionAction::FinishShutdown => self.apply_finish_shutdown(state)?,
        };
        Some(next)
    }
}

impl TemporalSpec for ManagerRuntimeProjectionSpec {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
        self.system().invariants()
    }
}

impl ModelCaseSource for ManagerRuntimeProjectionSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_check_deadlocks(false)]
    }
}

#[nirvash_projection_contract(
    probe_state = ManagerRuntimeProbeState,
    probe_output = ManagerRuntimeProbeOutput,
    summary_state = ManagerRuntimeStateSummary,
    summary_output = ManagerRuntimeOutputSummary,
    summarize_state = summarize_manager_runtime_state,
    summarize_output = summarize_manager_runtime_output,
    abstract_state = abstract_manager_runtime_state,
    abstract_output = abstract_manager_runtime_output
)]
impl ProtocolConformanceSpec for ManagerRuntimeProjectionSpec {
    type ExpectedOutput = Vec<SystemEffect>;

    fn expected_output(
        &self,
        _prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        if matches!(action, ManagerRuntimeProjectionAction::FinishShutdown) && next.is_some() {
            vec![SystemEffect::ShutdownComplete]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager_runtime::{ManagerRuntimePhase, TaskState};
    #[test]
    fn boot_projection_reaches_listening_state() {
        let spec = ManagerRuntimeProjectionSpec::new();
        let state = spec
            .transition(
                &spec.initial_state(),
                &ManagerRuntimeProjectionAction::LoadExistingConfig,
            )
            .expect("config load should be allowed");
        let state = spec
            .transition(
                &state,
                &ManagerRuntimeProjectionAction::RunPluginGcSucceeded,
            )
            .expect("plugin gc should be allowed");
        let state = spec
            .transition(
                &state,
                &ManagerRuntimeProjectionAction::RunBootRestoreSucceeded,
            )
            .expect("boot restore should be allowed");

        assert!(matches!(
            state.manager.phase,
            ManagerRuntimePhase::Listening
        ));
        assert!(matches!(state.manager.plugin_gc, TaskState::Succeeded));
        assert!(matches!(state.manager.boot_restore, TaskState::Succeeded));
    }

    #[test]
    fn shutdown_projection_finishes_with_shutdown_effect() {
        let spec = ManagerRuntimeProjectionSpec::new();
        let state = spec
            .transition(
                &spec.initial_state(),
                &ManagerRuntimeProjectionAction::LoadExistingConfig,
            )
            .expect("config load should be allowed");
        let state = spec
            .transition(
                &state,
                &ManagerRuntimeProjectionAction::RunPluginGcSucceeded,
            )
            .expect("plugin gc should be allowed");
        let state = spec
            .transition(
                &state,
                &ManagerRuntimeProjectionAction::RunBootRestoreSucceeded,
            )
            .expect("boot restore should be allowed");
        let state = spec
            .transition(&state, &ManagerRuntimeProjectionAction::BeginShutdown)
            .expect("shutdown should begin");
        let state = spec
            .transition(
                &state,
                &ManagerRuntimeProjectionAction::StopServicesGraceful,
            )
            .expect("graceful stop should advance");
        let state = spec
            .transition(&state, &ManagerRuntimeProjectionAction::StopMaintenance)
            .expect("maintenance stop should advance");
        let next = spec
            .transition(&state, &ManagerRuntimeProjectionAction::FinishShutdown)
            .expect("finish should be allowed");

        assert!(matches!(next.manager.phase, ManagerRuntimePhase::Stopped));
        assert_eq!(
            spec.expected_output(
                &state,
                &ManagerRuntimeProjectionAction::FinishShutdown,
                Some(&next)
            ),
            vec![SystemEffect::ShutdownComplete]
        );
    }
}
