use imagod_spec::{LogsOutputSummary, LogsStateSummary};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature};

use crate::{
    atoms::{LogChunkAtom, RequestKindAtom, ServiceAtom, SessionAtom, StreamAtom},
    deploy::DeployAction,
    deploy::DeployState,
    session_auth::SessionAuthAction,
    summary_mapping::system_effects,
    supervision::SupervisionAction,
    supervision::SupervisionState,
    system::{SystemAtomicAction, SystemEffect, SystemSpec, SystemState},
    wire_protocol::WireProtocolAction,
};

/// Logs ack/chunk/end surface projected from the unified `system` spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum LogsProjectionAction {
    /// Observe `logs.request` ack.
    LogsRequest,
    /// Observe one log datagram chunk.
    LogsChunk,
    /// Observe one log stream end datagram.
    LogsEnd,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LogsProjectionSpec;

impl LogsProjectionSpec {
    pub const fn new() -> Self {
        Self
    }

    fn system(self) -> SystemSpec {
        SystemSpec::new()
    }

    fn apply_atomic(self, state: &SystemState, action: SystemAtomicAction) -> SystemState {
        self.system()
            .transition(
                state,
                &nirvash_core::concurrent::ConcurrentAction::from_atomic(action),
            )
            .expect("projection seed state should admit delegated action")
    }

    pub fn initial_state(self) -> SystemState {
        let state = self.system().initial_state();
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0)),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0)),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0)),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0)),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0)),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(
                ServiceAtom::Service0,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(
                ServiceAtom::Service0,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(
                ServiceAtom::Service0,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::StartServing(ServiceAtom::Service0)),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream1,
                RequestKindAtom::LogsRequest,
            )),
        )
    }

    fn wire_action(self, action: LogsProjectionAction) -> WireProtocolAction {
        match action {
            LogsProjectionAction::LogsRequest => {
                WireProtocolAction::LogsRequest(StreamAtom::Stream1)
            }
            LogsProjectionAction::LogsChunk => {
                WireProtocolAction::LogsChunk(StreamAtom::Stream1, LogChunkAtom::Chunk0)
            }
            LogsProjectionAction::LogsEnd => WireProtocolAction::LogsEnd(StreamAtom::Stream1),
        }
    }
}

impl TransitionSystem for LogsProjectionSpec {
    type State = SystemState;
    type Action = LogsProjectionAction;

    fn name(&self) -> &'static str {
        "logs_projection"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        if matches!(action, LogsProjectionAction::LogsChunk)
            && state
                .wire
                .saw_log_chunk(StreamAtom::Stream1, LogChunkAtom::Chunk0)
        {
            return None;
        }
        self.system().transition(
            state,
            &nirvash_core::concurrent::ConcurrentAction::from_atomic(SystemAtomicAction::Wire(
                self.wire_action(*action),
            )),
        )
    }
}

impl TemporalSpec for LogsProjectionSpec {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
        self.system().invariants()
    }
}

impl ModelCaseSource for LogsProjectionSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_check_deadlocks(false)]
    }
}

impl ProtocolConformanceSpec for LogsProjectionSpec {
    type ExpectedOutput = Vec<SystemEffect>;
    type SummaryState = LogsStateSummary;
    type SummaryOutput = LogsOutputSummary;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        self.system().expected_output(
            prev,
            &nirvash_core::concurrent::ConcurrentAction::from_atomic(SystemAtomicAction::Wire(
                self.wire_action(*action),
            )),
            next,
        )
    }

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
        let mut state = self.initial_state();
        state.deploy = DeployState::from_logs_summary(summary);
        state.supervision = SupervisionState::from_logs_summary(summary);
        state.wire = crate::wire_protocol::WireProtocolState::from_logs_summary(summary);
        state
    }

    fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
        system_effects(&summary.effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_has_running_service_and_logs_authorization() {
        let state = LogsProjectionSpec::new().initial_state();

        assert!(state.supervision.service_is_running(ServiceAtom::Service0));
        assert!(
            state
                .session_auth
                .stream_authorized(StreamAtom::Stream1, RequestKindAtom::LogsRequest)
        );
    }

    #[test]
    fn logs_projection_reaches_ack_chunk_and_end() {
        let spec = LogsProjectionSpec::new();
        let state = spec
            .transition(&spec.initial_state(), &LogsProjectionAction::LogsRequest)
            .expect("logs.request should be allowed");
        let state = spec
            .transition(&state, &LogsProjectionAction::LogsChunk)
            .expect("logs.chunk should be allowed");
        let state = spec
            .transition(&state, &LogsProjectionAction::LogsEnd)
            .expect("logs.end should be allowed");

        assert!(state.wire.logs_acknowledged(StreamAtom::Stream1));
        assert!(state.wire.log_stream_ended(StreamAtom::Stream1));
    }

    #[test]
    fn logs_projection_allows_only_one_observable_chunk_step() {
        let spec = LogsProjectionSpec::new();
        let state = spec
            .transition(&spec.initial_state(), &LogsProjectionAction::LogsRequest)
            .expect("logs.request should be allowed");
        let state = spec
            .transition(&state, &LogsProjectionAction::LogsChunk)
            .expect("first logs.chunk should be allowed");

        assert!(
            spec.transition(&state, &LogsProjectionAction::LogsChunk)
                .is_none(),
            "projection should collapse repeated runtime chunks into one observable step",
        );
    }
}
