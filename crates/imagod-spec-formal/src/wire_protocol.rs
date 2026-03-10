use nirvash_core::{
    ActionConstraint, Fairness, Ltl, ModelCase, ModelCheckConfig, RelSet, Relation2,
    Signature as _, StatePredicate, StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelationalState, action_constraint, fairness, invariant, property,
    subsystem_spec,
};

use crate::atoms::{CommandEventAtom, LogChunkAtom, RequestKindAtom, StreamAtom};
use crate::summary_mapping::request_kind_atom;

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
pub struct WireProtocolState {
    requests: Relation2<StreamAtom, RequestKindAtom>,
    responses: Relation2<StreamAtom, RequestKindAtom>,
    command_events: Relation2<StreamAtom, CommandEventAtom>,
    log_follow_streams: RelSet<StreamAtom>,
    log_chunks: Relation2<StreamAtom, LogChunkAtom>,
    log_ended: RelSet<StreamAtom>,
}

impl WireProtocolState {
    pub fn from_router_summary(summary: &imagod_spec::RouterStateSummary) -> Self {
        let mut state = Self {
            requests: Relation2::empty(),
            responses: Relation2::empty(),
            command_events: Relation2::empty(),
            log_follow_streams: RelSet::empty(),
            log_chunks: Relation2::empty(),
            log_ended: RelSet::empty(),
        };
        if let Some(kind) = summary.request {
            state
                .requests
                .insert(StreamAtom::Stream0, request_kind_atom(kind));
            state
                .responses
                .insert(StreamAtom::Stream0, request_kind_atom(kind));
        }
        state
    }

    pub fn from_logs_summary(summary: &imagod_spec::LogsStateSummary) -> Self {
        let mut state = Self {
            requests: Relation2::empty(),
            responses: Relation2::empty(),
            command_events: Relation2::empty(),
            log_follow_streams: RelSet::empty(),
            log_chunks: Relation2::empty(),
            log_ended: RelSet::empty(),
        };
        if summary.acknowledged {
            state
                .requests
                .insert(StreamAtom::Stream1, RequestKindAtom::LogsRequest);
            state
                .responses
                .insert(StreamAtom::Stream1, RequestKindAtom::LogsRequest);
            state.log_follow_streams.insert(StreamAtom::Stream1);
        }
        if summary.chunk_seen {
            state
                .log_chunks
                .insert(StreamAtom::Stream1, LogChunkAtom::Chunk0);
        }
        if summary.ended {
            state.log_ended.insert(StreamAtom::Stream1);
        }
        state
    }

    pub fn saw_request(&self, stream: StreamAtom, kind: RequestKindAtom) -> bool {
        self.requests.contains(&stream, &kind)
    }

    pub fn logs_acknowledged(&self, stream: StreamAtom) -> bool {
        self.responses
            .contains(&stream, &RequestKindAtom::LogsRequest)
    }

    pub fn log_stream_ended(&self, stream: StreamAtom) -> bool {
        self.log_ended.contains(&stream)
    }

    pub fn saw_log_chunk(&self, stream: StreamAtom, chunk: LogChunkAtom) -> bool {
        self.log_chunks.contains(&stream, &chunk)
    }

    pub fn saw_command_event(&self, stream: StreamAtom, event: CommandEventAtom) -> bool {
        self.command_events.contains(&stream, &event)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, nirvash_macros::Signature, ActionVocabulary)]
pub enum WireProtocolAction {
    /// Observe one hello negotiation request/response.
    HelloNegotiate(StreamAtom),
    /// Observe one deploy.prepare request/response.
    DeployPrepare(StreamAtom),
    /// Observe one artifact.push request/response.
    ArtifactPush(StreamAtom),
    /// Observe one artifact.commit request/response.
    ArtifactCommit(StreamAtom),
    /// Observe one command.start request/response.
    CommandStart(StreamAtom),
    /// Observe one command.event emission.
    CommandEvent(StreamAtom, CommandEventAtom),
    /// Observe one state.request/state.response pair.
    StateRequest(StreamAtom),
    /// Observe one services.list request/response pair.
    ServicesList(StreamAtom),
    /// Observe one command.cancel request/response pair.
    CommandCancel(StreamAtom),
    /// Observe one logs.request ack.
    LogsRequest(StreamAtom),
    /// Observe one log datagram chunk.
    LogsChunk(StreamAtom, LogChunkAtom),
    /// Observe one logs.end datagram.
    LogsEnd(StreamAtom),
    /// Observe one rpc.invoke request/response pair.
    RpcInvoke(StreamAtom),
    /// Observe one bindings.cert.upload request/response pair.
    BindingsCertUpload(StreamAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WireProtocolSpec;

impl WireProtocolSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> WireProtocolState {
        WireProtocolState {
            requests: Relation2::empty(),
            responses: Relation2::empty(),
            command_events: Relation2::empty(),
            log_follow_streams: RelSet::empty(),
            log_chunks: Relation2::empty(),
            log_ended: RelSet::empty(),
        }
    }
}

fn wire_protocol_model_cases() -> Vec<ModelCase<WireProtocolState, WireProtocolAction>> {
    vec![
        ModelCase::default()
            .with_checker_config(ModelCheckConfig {
                exploration: nirvash_core::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(96),
                max_transitions: Some(384),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_doc_checker_config(ModelCheckConfig {
                exploration: nirvash_core::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(64),
                max_transitions: Some(256),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_check_deadlocks(false),
    ]
}

fn stream_for_action(action: WireProtocolAction) -> StreamAtom {
    match action {
        WireProtocolAction::HelloNegotiate(stream)
        | WireProtocolAction::DeployPrepare(stream)
        | WireProtocolAction::ArtifactPush(stream)
        | WireProtocolAction::ArtifactCommit(stream)
        | WireProtocolAction::CommandStart(stream)
        | WireProtocolAction::CommandEvent(stream, _)
        | WireProtocolAction::StateRequest(stream)
        | WireProtocolAction::ServicesList(stream)
        | WireProtocolAction::CommandCancel(stream)
        | WireProtocolAction::LogsRequest(stream)
        | WireProtocolAction::LogsChunk(stream, _)
        | WireProtocolAction::LogsEnd(stream)
        | WireProtocolAction::RpcInvoke(stream)
        | WireProtocolAction::BindingsCertUpload(stream) => stream,
    }
}

#[action_constraint(WireProtocolSpec, cases("default"))]
fn stream0_only() -> ActionConstraint<WireProtocolState, WireProtocolAction> {
    ActionConstraint::new("stream0_only", |_, action, _| {
        stream_for_action(*action) == StreamAtom::Stream0
    })
}

fn request_kind_for_action(action: WireProtocolAction) -> Option<RequestKindAtom> {
    match action {
        WireProtocolAction::HelloNegotiate(_) => Some(RequestKindAtom::HelloNegotiate),
        WireProtocolAction::DeployPrepare(_) => Some(RequestKindAtom::DeployPrepare),
        WireProtocolAction::ArtifactPush(_) => Some(RequestKindAtom::ArtifactPush),
        WireProtocolAction::ArtifactCommit(_) => Some(RequestKindAtom::ArtifactCommit),
        WireProtocolAction::CommandStart(_) => Some(RequestKindAtom::CommandStart),
        WireProtocolAction::CommandEvent(_, _) => Some(RequestKindAtom::CommandEvent),
        WireProtocolAction::StateRequest(_) => Some(RequestKindAtom::StateRequest),
        WireProtocolAction::ServicesList(_) => Some(RequestKindAtom::ServicesList),
        WireProtocolAction::CommandCancel(_) => Some(RequestKindAtom::CommandCancel),
        WireProtocolAction::LogsRequest(_) => Some(RequestKindAtom::LogsRequest),
        WireProtocolAction::LogsChunk(_, _) => Some(RequestKindAtom::LogsChunk),
        WireProtocolAction::LogsEnd(_) => Some(RequestKindAtom::LogsEnd),
        WireProtocolAction::RpcInvoke(_) => Some(RequestKindAtom::RpcInvoke),
        WireProtocolAction::BindingsCertUpload(_) => Some(RequestKindAtom::BindingsCertUpload),
    }
}

#[invariant(WireProtocolSpec)]
fn one_request_kind_per_stream() -> StatePredicate<WireProtocolState> {
    StatePredicate::new("one_request_kind_per_stream", |state| {
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                state
                    .requests
                    .domain()
                    .items()
                    .into_iter()
                    .filter(|item| item == &stream)
                    .count()
                    <= 1
            })
    })
}

#[invariant(WireProtocolSpec)]
fn logs_flow_requires_ack() -> StatePredicate<WireProtocolState> {
    StatePredicate::new("logs_flow_requires_ack", |state| {
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                (!state.log_follow_streams.contains(&stream)
                    && !state.log_chunks.domain().contains(&stream)
                    && !state.log_ended.contains(&stream))
                    || state.logs_acknowledged(stream)
            })
    })
}

#[invariant(WireProtocolSpec)]
fn command_events_require_command_start() -> StatePredicate<WireProtocolState> {
    StatePredicate::new("command_events_require_command_start", |state| {
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                !state.command_events.domain().contains(&stream)
                    || state.saw_request(stream, RequestKindAtom::CommandStart)
            })
    })
}

#[property(WireProtocolSpec)]
fn logs_request_leads_to_logs_end() -> Ltl<WireProtocolState, WireProtocolAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("logs_acknowledged", |state| {
            state
                .responses
                .range()
                .contains(&RequestKindAtom::LogsRequest)
        })),
        Ltl::pred(StatePredicate::new("logs_follow_active", |state| {
            state.log_follow_streams.some()
        })),
    )
}

#[fairness(WireProtocolSpec)]
fn logs_progress_fairness() -> Fairness<WireProtocolState, WireProtocolAction> {
    Fairness::weak(StepPredicate::new("logs_progress", |_, action, next| {
        matches!(
            action,
            WireProtocolAction::LogsChunk(_, _) | WireProtocolAction::LogsEnd(_)
        ) && (next.log_chunks.some() || next.log_ended.some())
    }))
}

#[subsystem_spec(model_cases(wire_protocol_model_cases))]
impl TransitionSystem for WireProtocolSpec {
    type State = WireProtocolState;
    type Action = WireProtocolAction;

    fn name(&self) -> &'static str {
        "wire_protocol"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_wire_protocol(state, action)
    }
}

#[nirvash_macros::formal_tests(spec = WireProtocolSpec)]
const _: () = ();

fn transition_wire_protocol(
    prev: &WireProtocolState,
    action: &WireProtocolAction,
) -> Option<WireProtocolState> {
    let mut candidate = prev.clone();
    let stream = stream_for_action(*action);
    let allowed = match action {
        WireProtocolAction::CommandEvent(_, event)
            if prev.saw_request(stream, RequestKindAtom::CommandStart) =>
        {
            candidate.command_events.insert(stream, *event);
            true
        }
        WireProtocolAction::CommandEvent(_, _) => false,
        WireProtocolAction::LogsChunk(_, chunk)
            if prev.logs_acknowledged(stream) && !prev.log_ended.contains(&stream) =>
        {
            candidate.log_chunks.insert(stream, *chunk);
            true
        }
        WireProtocolAction::LogsChunk(_, _) => false,
        WireProtocolAction::LogsEnd(_)
            if prev.logs_acknowledged(stream) && !prev.log_ended.contains(&stream) =>
        {
            candidate.log_ended.insert(stream);
            true
        }
        WireProtocolAction::LogsEnd(_) => false,
        _ => {
            let kind = request_kind_for_action(*action)?;
            if prev.requests.domain().contains(&stream) {
                false
            } else {
                candidate.requests.insert(stream, kind);
                candidate.responses.insert(stream, kind);
                if matches!(action, WireProtocolAction::LogsRequest(_)) {
                    candidate.log_follow_streams.insert(stream);
                }
                true
            }
        }
    };

    allowed.then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_follow_requires_ack_before_chunking() {
        let spec = WireProtocolSpec::new();
        let state = spec.initial_state();

        assert!(
            spec.transition(
                &state,
                &WireProtocolAction::LogsChunk(StreamAtom::Stream0, LogChunkAtom::Chunk0),
            )
            .is_none()
        );
        let acknowledged = spec
            .transition(
                &state,
                &WireProtocolAction::LogsRequest(StreamAtom::Stream0),
            )
            .expect("logs request should ack");
        assert!(
            spec.transition(
                &acknowledged,
                &WireProtocolAction::LogsChunk(StreamAtom::Stream0, LogChunkAtom::Chunk0),
            )
            .is_some()
        );
    }
}
