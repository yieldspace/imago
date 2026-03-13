#[cfg(test)]
use imagod_spec::{ContractEffectSummary, SummaryLogChunk, SummaryRequestKind, SummaryStreamId};
use imagod_spec::{LogsOutputSummary, LogsProbeOutput, LogsProbeState, LogsStateSummary};
use nirvash::BoolExpr;
use nirvash_conformance::ProtocolConformanceSpec;
use nirvash_lower::{FrontendSpec, ModelInstance, TemporalSpec};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    SymbolicEncoding as FormalSymbolicEncoding, nirvash_projection_model,
};

use crate::{
    atoms::{LogChunkAtom, RequestKindAtom, ServiceAtom, SessionAtom, StreamAtom},
    deploy::DeployAction,
    deploy::DeployState,
    session_auth::SessionAuthAction,
    supervision::SupervisionAction,
    supervision::SupervisionState,
    system::{SystemAtomicAction, SystemEffect, SystemSpec, SystemState},
    wire_protocol::WireProtocolAction,
};

/// Logs ack/chunk/end surface projected from the unified `system` spec.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    ActionVocabulary,
)]
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
            .transition(state, &action)
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

    fn state_from_summary(self, summary: &LogsStateSummary) -> SystemState {
        let mut state = self.initial_state();
        state.deploy = DeployState::from_logs_summary(summary);
        state.supervision = SupervisionState::from_logs_summary(summary);
        state.wire = crate::wire_protocol::WireProtocolState::from_logs_summary(summary);
        state
    }
}

impl FrontendSpec for LogsProjectionSpec {
    type State = SystemState;
    type Action = LogsProjectionAction;

    fn frontend_name(&self) -> &'static str {
        "logs_projection"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        if matches!(action, LogsProjectionAction::LogsChunk)
            && state
                .wire
                .saw_log_chunk(StreamAtom::Stream1, LogChunkAtom::Chunk0)
        {
            return None;
        }
        self.system()
            .transition(state, &SystemAtomicAction::Wire(self.wire_action(*action)))
            .map(|next| {
                let acknowledged = next.wire.logs_acknowledged(StreamAtom::Stream1);
                let completed = next.wire.log_stream_ended(StreamAtom::Stream1);
                let probe = LogsProbeState {
                    service_running: next.supervision.service_is_running(ServiceAtom::Service0),
                    logs_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream1, RequestKindAtom::LogsRequest),
                    stream_open: acknowledged && !completed,
                    chunk_pending: acknowledged
                        && !completed
                        && !next
                            .wire
                            .saw_log_chunk(StreamAtom::Stream1, LogChunkAtom::Chunk0),
                    completed,
                };
                let summary = <Self as ProtocolConformanceSpec>::summarize_state(self, &probe);
                <Self as ProtocolConformanceSpec>::abstract_state(self, &summary)
            })
    }

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        vec![ModelInstance::default().with_check_deadlocks(false)]
    }
}

impl TemporalSpec for LogsProjectionSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        self.system().invariants()
    }
}

#[cfg(test)]
fn logs_probe_state_domain() -> nirvash::BoundedDomain<LogsProbeState> {
    <LogsProbeState as nirvash_lower::FiniteModelDomain>::bounded_domain()
}

#[cfg(test)]
fn logs_summary_output_domain() -> nirvash::BoundedDomain<LogsOutputSummary> {
    nirvash::BoundedDomain::new(vec![
        LogsOutputSummary {
            effects: vec![
                ContractEffectSummary::RequestObserved(
                    SummaryStreamId::Stream1,
                    SummaryRequestKind::LogsRequest,
                ),
                ContractEffectSummary::Response(
                    SummaryStreamId::Stream1,
                    SummaryRequestKind::LogsRequest,
                ),
            ],
        },
        LogsOutputSummary {
            effects: vec![ContractEffectSummary::LogChunk(
                SummaryStreamId::Stream1,
                SummaryLogChunk::Chunk0,
            )],
        },
        LogsOutputSummary {
            effects: vec![ContractEffectSummary::LogsEnd(SummaryStreamId::Stream1)],
        },
    ])
}

nirvash_projection_model! {
    probe_state = LogsProbeState,
    probe_output = LogsProbeOutput,
    summary_state = LogsStateSummary,
    summary_output = LogsOutputSummary,
    abstract_state = SystemState,
    expected_output = Vec<SystemEffect>,
    probe_state_domain = logs_probe_state_domain,
    summary_output_domain = logs_summary_output_domain,
    state_seed = spec.initial_state(),
    state_summary {
        service_running <= probe.service_running,
        logs_authorized <= probe.logs_authorized,
        stream_open <= probe.stream_open,
        chunk_pending <= probe.chunk_pending,
        completed <= probe.completed,
    }
    output_summary {
        effects <= probe.output.effects.clone(),
    }
    state_abstract {
        state <= spec.state_from_summary(summary),
    }
    output_abstract {
        imagod_spec::ContractEffectSummary::RequestObserved(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::Response(_, _) => crate::summary_mapping::system_effect(effect)
            .expect("logs projection response should map to SystemEffect"),
        imagod_spec::ContractEffectSummary::AuthorizationGranted(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::CommandEvent(_, _) => crate::summary_mapping::system_effect(effect)
            .expect("logs projection command event should map to SystemEffect"),
        effect @ imagod_spec::ContractEffectSummary::LogChunk(_, _) => crate::summary_mapping::system_effect(effect)
            .expect("logs projection log chunk should map to SystemEffect"),
        effect @ imagod_spec::ContractEffectSummary::LogsEnd(_) => crate::summary_mapping::system_effect(effect)
            .expect("logs projection logs end should map to SystemEffect"),
        effect @ imagod_spec::ContractEffectSummary::AuthorizationRejected(_, _) => crate::summary_mapping::system_effect(effect)
            .expect("logs projection authorization rejection should map to SystemEffect"),
        imagod_spec::ContractEffectSummary::LocalRpcResolved(_) => drop,
        imagod_spec::ContractEffectSummary::LocalRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcConnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcCompleted(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDisconnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::TaskMilestone(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::ShutdownComplete => crate::summary_mapping::system_effect(effect)
            .expect("logs projection shutdown completion should map to SystemEffect"),
    }
    impl ProtocolConformanceSpec for LogsProjectionSpec {
        type ExpectedOutput = Vec<SystemEffect>;

        fn expected_output(
            &self,
            prev: &Self::State,
            action: &Self::Action,
            next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            self.system().expected_output(
                prev,
                &SystemAtomicAction::Wire(self.wire_action(*action)),
                next,
            )
        }
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
