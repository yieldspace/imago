use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    concurrent::ConcurrentAction, conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature};

use crate::{
    manager_runtime::{ManagerRuntimeAction, ManagerRuntimeSpec},
    session_transport::{SessionTransportAction, SessionTransportSpec},
    shutdown_flow::{ShutdownFlowAction, ShutdownFlowSpec, ShutdownPhase},
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ManagerRuntimeProjectionObservedState {
    pub trace: Vec<ManagerRuntimeProjectionAction>,
}

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

impl ProtocolConformanceSpec for ManagerRuntimeProjectionSpec {
    type ExpectedOutput = Vec<SystemEffect>;
    type ObservedState = ManagerRuntimeProjectionObservedState;
    type ObservedOutput = Vec<SystemEffect>;

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

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State {
        observed
            .trace
            .iter()
            .fold(self.initial_state(), |state, action| {
                self.transition(&state, action)
                    .expect("manager runtime projection trace should stay valid")
            })
    }

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput {
        observed.clone()
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
