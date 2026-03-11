use imagod_spec::{
    CommandOutputSummary as RuntimeCommandOutputSummary,
    CommandProbeOutput as RuntimeCommandProbeOutput, CommandProbeState as RuntimeCommandProbeState,
    CommandStateSummary as RuntimeCommandStateSummary,
};
use nirvash_core::{
    ActionVocabulary, BoolExpr, ModelCase, ModelCaseSource, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::nirvash_projection_contract;

use crate::{
    CommandProtocolAction,
    command_protocol::{CommandProtocolExpectedOutput, CommandProtocolSpec, CommandProtocolState},
    system::{SystemAtomicAction, SystemSpec, SystemState},
};

/// Projection spec that replays command-runtime behavior against the unified `system` state.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandProjectionSpec;

impl CommandProjectionSpec {
    pub const fn new() -> Self {
        Self
    }

    fn system(self) -> SystemSpec {
        SystemSpec::new()
    }

    pub fn initial_state(self) -> SystemState {
        self.system().initial_state()
    }

    fn command_observed_state(self, observed: &RuntimeCommandStateSummary) -> CommandProtocolState {
        CommandProtocolState {
            tracked: observed.tracked,
            lifecycle_state: observed.lifecycle_state,
            cancel_requested: observed.cancel_requested,
            phase: observed.phase,
        }
    }
}

fn summarize_command_state(probe: &RuntimeCommandProbeState) -> RuntimeCommandStateSummary {
    *probe
}

fn summarize_command_output(probe: &RuntimeCommandProbeOutput) -> RuntimeCommandOutputSummary {
    probe.clone()
}

fn abstract_command_state(
    spec: &CommandProjectionSpec,
    observed: &RuntimeCommandStateSummary,
) -> SystemState {
    let mut state = spec.initial_state();
    state.command = spec.command_observed_state(observed);
    state
}

fn abstract_command_output(
    _spec: &CommandProjectionSpec,
    observed: &RuntimeCommandOutputSummary,
) -> CommandProtocolExpectedOutput {
    <CommandProtocolSpec as ProtocolConformanceSpec>::abstract_output(
        &CommandProtocolSpec::new(),
        observed,
    )
}

impl TransitionSystem for CommandProjectionSpec {
    type State = SystemState;
    type Action = CommandProtocolAction;

    fn name(&self) -> &'static str {
        "command_projection"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <CommandProtocolAction as ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.system()
            .transition(state, &SystemAtomicAction::Command(action.clone()))
    }
}

impl TemporalSpec for CommandProjectionSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        self.system().invariants()
    }
}

impl ModelCaseSource for CommandProjectionSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_check_deadlocks(false)]
    }
}

#[nirvash_projection_contract(
    probe_state = RuntimeCommandProbeState,
    probe_output = RuntimeCommandProbeOutput,
    summary_state = RuntimeCommandStateSummary,
    summary_output = RuntimeCommandOutputSummary,
    summarize_state = summarize_command_state,
    summarize_output = summarize_command_output,
    abstract_state = abstract_command_state,
    abstract_output = abstract_command_output
)]
impl ProtocolConformanceSpec for CommandProjectionSpec {
    type ExpectedOutput = CommandProtocolExpectedOutput;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        <CommandProtocolSpec as ProtocolConformanceSpec>::expected_output(
            &CommandProtocolSpec::new(),
            &prev.command,
            action,
            next.map(|state| &state.command),
        )
    }
}

#[cfg(test)]
mod tests {
    use imagod_spec::{CommandKind, OperationPhase};

    use super::*;

    #[test]
    fn initial_state_starts_from_listening_system() {
        let state = CommandProjectionSpec::new().initial_state();
        assert!(!state.command.tracked);
        assert!(matches!(
            state.manager.phase,
            crate::manager_runtime::ManagerRuntimePhase::Listening
        ));
    }

    #[test]
    fn start_updates_only_command_projection() {
        let spec = CommandProjectionSpec::new();
        let state = spec
            .transition(
                &spec.initial_state(),
                &CommandProtocolAction::Start(CommandKind::Deploy),
            )
            .expect("start should be allowed");

        assert!(state.command.tracked);
        assert_eq!(state.command.phase, Some(OperationPhase::Starting));
        assert!(matches!(
            state.manager.phase,
            crate::manager_runtime::ManagerRuntimePhase::Listening
        ));
        assert!(matches!(
            state.shutdown.phase,
            crate::shutdown_flow::ShutdownPhase::Idle
        ));
    }
}
