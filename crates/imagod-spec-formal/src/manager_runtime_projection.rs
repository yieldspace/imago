#[cfg(test)]
use imagod_spec::{ContractEffectSummary, SummaryTaskKind, SummaryTaskState};
use imagod_spec::{
    ManagerRuntimeOutputSummary, ManagerRuntimeProbeOutput, ManagerRuntimeProbeState,
    ManagerRuntimeStateSummary,
};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    concurrent::ConcurrentAction, conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature, nirvash_projection_model};

use crate::{
    manager_runtime::{ManagerRuntimeAction, ManagerRuntimeSpec},
    session_transport::{SessionTransportAction, SessionTransportSpec},
    shutdown_flow::{ShutdownFlowAction, ShutdownFlowSpec, ShutdownPhase},
    summary_mapping::{manager_runtime_phase, shutdown_phase, system_effect},
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
        if matches!(
            state.manager.phase,
            crate::manager_runtime::ManagerRuntimePhase::Stopped
        ) && matches!(state.shutdown.phase, ShutdownPhase::Completed)
        {
            return Some(state.clone());
        }
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

fn manager_runtime_summary_from_state(state: &SystemState) -> ManagerRuntimeStateSummary {
    let manager_shutdown_started = matches!(
        state.manager.phase,
        crate::manager_runtime::ManagerRuntimePhase::ShutdownRequested
            | crate::manager_runtime::ManagerRuntimePhase::Stopped
    );
    let manager_stopped = matches!(
        state.manager.phase,
        crate::manager_runtime::ManagerRuntimePhase::Stopped
    );

    ManagerRuntimeStateSummary {
        config_loaded: state.manager.config_loaded,
        created_default: state.manager.created_default,
        manager_phase: match state.manager.phase {
            crate::manager_runtime::ManagerRuntimePhase::Booting => {
                imagod_spec::SummaryManagerRuntimePhase::Booting
            }
            crate::manager_runtime::ManagerRuntimePhase::ConfigReady => {
                imagod_spec::SummaryManagerRuntimePhase::ConfigReady
            }
            crate::manager_runtime::ManagerRuntimePhase::Restoring => {
                imagod_spec::SummaryManagerRuntimePhase::Restoring
            }
            crate::manager_runtime::ManagerRuntimePhase::Listening => {
                imagod_spec::SummaryManagerRuntimePhase::Listening
            }
            crate::manager_runtime::ManagerRuntimePhase::ShutdownRequested => {
                imagod_spec::SummaryManagerRuntimePhase::ShutdownRequested
            }
            crate::manager_runtime::ManagerRuntimePhase::Stopped => {
                imagod_spec::SummaryManagerRuntimePhase::Stopped
            }
        },
        listening: matches!(
            state.manager.phase,
            crate::manager_runtime::ManagerRuntimePhase::Listening
        ),
        manager_shutdown_started,
        manager_stopped,
        session_shutdown_requested: state.session.shutdown_requested,
        shutdown: imagod_spec::ShutdownStateSummary {
            phase: match state.shutdown.phase {
                ShutdownPhase::Idle => imagod_spec::SummaryShutdownPhase::Idle,
                ShutdownPhase::SignalReceived => imagod_spec::SummaryShutdownPhase::SignalReceived,
                ShutdownPhase::DrainingSessions => {
                    imagod_spec::SummaryShutdownPhase::DrainingSessions
                }
                ShutdownPhase::StoppingServices => {
                    imagod_spec::SummaryShutdownPhase::StoppingServices
                }
                ShutdownPhase::StoppingMaintenance => {
                    imagod_spec::SummaryShutdownPhase::StoppingMaintenance
                }
                ShutdownPhase::Completed => imagod_spec::SummaryShutdownPhase::Completed,
            },
            accepts_stopped: state.shutdown.accepts_stopped,
            sessions_drained: state.shutdown.sessions_drained,
            services_stopped: state.shutdown.services_stopped,
            maintenance_stopped: state.shutdown.maintenance_stopped,
            forced_stop_attempted: state.shutdown.forced_stop_attempted,
        },
    }
}

fn normalize_manager_runtime_state(
    spec: ManagerRuntimeProjectionSpec,
    state: SystemState,
) -> SystemState {
    let summary = manager_runtime_summary_from_state(&state);
    <ManagerRuntimeProjectionSpec as ProtocolConformanceSpec>::abstract_state(&spec, &summary)
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
        Some(normalize_manager_runtime_state(*self, next))
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

#[cfg(test)]
fn manager_runtime_probe_state_domain() -> nirvash_core::BoundedDomain<ManagerRuntimeProbeState> {
    <ManagerRuntimeProbeState as nirvash_core::Signature>::bounded_domain()
}

#[cfg(test)]
fn manager_runtime_summary_output_domain()
-> nirvash_core::BoundedDomain<ManagerRuntimeOutputSummary> {
    nirvash_core::BoundedDomain::new(vec![
        ManagerRuntimeOutputSummary::default(),
        ManagerRuntimeOutputSummary {
            effects: vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::PluginGc,
                SummaryTaskState::Succeeded,
            )],
        },
        ManagerRuntimeOutputSummary {
            effects: vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::PluginGc,
                SummaryTaskState::Failed,
            )],
        },
        ManagerRuntimeOutputSummary {
            effects: vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::BootRestore,
                SummaryTaskState::Succeeded,
            )],
        },
        ManagerRuntimeOutputSummary {
            effects: vec![ContractEffectSummary::TaskMilestone(
                SummaryTaskKind::BootRestore,
                SummaryTaskState::Failed,
            )],
        },
        ManagerRuntimeOutputSummary {
            effects: vec![ContractEffectSummary::ShutdownComplete],
        },
    ])
}

nirvash_projection_model! {
    probe_state = ManagerRuntimeProbeState,
    probe_output = ManagerRuntimeProbeOutput,
    summary_state = ManagerRuntimeStateSummary,
    summary_output = ManagerRuntimeOutputSummary,
    abstract_state = SystemState,
    expected_output = Vec<SystemEffect>,
    probe_state_domain = manager_runtime_probe_state_domain,
    summary_output_domain = manager_runtime_summary_output_domain,
    state_seed = spec.initial_state(),
    state_summary {
        config_loaded <= probe.config_loaded,
        created_default <= probe.created_default,
        manager_phase <= probe.manager_phase,
        listening <= probe.listening,
        manager_shutdown_started <= probe.manager_shutdown_started,
        manager_stopped <= probe.manager_stopped,
        session_shutdown_requested <= probe.session_shutdown_requested,
        shutdown <= probe.shutdown,
    }
    output_summary {
        effects <= probe.output.effects.clone()
    }
    state_abstract {
        state.manager.config_loaded <= summary.config_loaded,
        state.manager.created_default <= summary.created_default,
        state.manager.phase <= manager_runtime_phase(summary.manager_phase),
        state.session.shutdown_requested <= summary.session_shutdown_requested,
        state.shutdown.phase <= shutdown_phase(summary.shutdown.phase),
        state.shutdown.accepts_stopped <= summary.shutdown.accepts_stopped,
        state.shutdown.sessions_drained <= summary.shutdown.sessions_drained,
        state.shutdown.services_stopped <= summary.shutdown.services_stopped,
        state.shutdown.maintenance_stopped <= summary.shutdown.maintenance_stopped,
        state.shutdown.forced_stop_attempted <= summary.shutdown.forced_stop_attempted,
    }
    output_abstract {
        imagod_spec::ContractEffectSummary::RequestObserved(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::Response(_, _) => system_effect(effect).expect("response effect should map"),
        effect @ imagod_spec::ContractEffectSummary::CommandEvent(_, _) => system_effect(effect).expect("command event should map"),
        effect @ imagod_spec::ContractEffectSummary::LogChunk(_, _) => system_effect(effect).expect("log chunk should map"),
        effect @ imagod_spec::ContractEffectSummary::LogsEnd(_) => system_effect(effect).expect("logs end should map"),
        imagod_spec::ContractEffectSummary::AuthorizationGranted(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::AuthorizationRejected(_, _) => system_effect(effect).expect("authorization rejection should map"),
        imagod_spec::ContractEffectSummary::LocalRpcResolved(_) => drop,
        imagod_spec::ContractEffectSummary::LocalRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcConnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcCompleted(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDisconnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::TaskMilestone(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::ShutdownComplete => system_effect(effect).expect("shutdown completion should map")
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager_runtime::ManagerRuntimePhase;
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
