use imagod_spec::{
    CommandOutputSummary as RuntimeCommandOutputSummary,
    CommandStateSummary as RuntimeCommandStateSummary,
};
use nirvash_core::{
    ActionVocabulary, ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    concurrent::ConcurrentAction, conformance::ProtocolConformanceSpec,
};

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
        self.system().transition(
            state,
            &ConcurrentAction::from_atomic(SystemAtomicAction::Command(action.clone())),
        )
    }
}

impl TemporalSpec for CommandProjectionSpec {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
        self.system().invariants()
    }
}

impl ModelCaseSource for CommandProjectionSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_check_deadlocks(false)]
    }
}

impl ProtocolConformanceSpec for CommandProjectionSpec {
    type ExpectedOutput = CommandProtocolExpectedOutput;
    type SummaryState = RuntimeCommandStateSummary;
    type SummaryOutput = RuntimeCommandOutputSummary;

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

    fn abstract_state(&self, observed: &Self::SummaryState) -> Self::State {
        let mut state = self.initial_state();
        state.command = self.command_observed_state(observed);
        state
    }

    fn abstract_output(&self, observed: &Self::SummaryOutput) -> Self::ExpectedOutput {
        <CommandProtocolSpec as ProtocolConformanceSpec>::abstract_output(
            &CommandProtocolSpec::new(),
            observed,
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
