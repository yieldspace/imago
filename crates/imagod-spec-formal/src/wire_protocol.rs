use nirvash_core::{
    BoolExpr, Fairness, Ltl, ModelBackend, ModelCase, ModelCheckConfig, RelSet, Relation2,
    Signature as _, StepExpr, TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelationalState, Signature as FormalSignature, action_constraint, fairness,
    invariant, nirvash_expr, nirvash_step_expr, nirvash_transition_program, property,
    subsystem_spec,
};

use crate::atoms::{CommandEventAtom, LogChunkAtom, RequestKindAtom, StreamAtom};
#[derive(Debug, Clone, PartialEq, Eq, FormalSignature, RelationalState)]
#[signature(custom)]
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
        let _ = summary;
        Self {
            requests: Relation2::empty(),
            responses: Relation2::empty(),
            command_events: Relation2::empty(),
            log_follow_streams: RelSet::empty(),
            log_chunks: Relation2::empty(),
            log_ended: RelSet::empty(),
        }
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
        if summary.stream_open || summary.completed {
            state
                .requests
                .insert(StreamAtom::Stream1, RequestKindAtom::LogsRequest);
            state
                .responses
                .insert(StreamAtom::Stream1, RequestKindAtom::LogsRequest);
        }
        if summary.stream_open {
            state.log_follow_streams.insert(StreamAtom::Stream1);
        }
        if !summary.chunk_pending && (summary.stream_open || summary.completed) {
            state
                .log_chunks
                .insert(StreamAtom::Stream1, LogChunkAtom::Chunk0);
        }
        if summary.completed {
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

nirvash_core::signature_spec!(
    WireProtocolStateSignatureSpec for WireProtocolState,
    representatives = crate::state_domain::reachable_state_domain(&WireProtocolSpec::new())
);

fn wire_protocol_model_cases() -> Vec<ModelCase<WireProtocolState, WireProtocolAction>> {
    vec![
        ModelCase::default()
            .with_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                exploration: nirvash_core::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(96),
                max_transitions: Some(384),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_doc_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
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
fn stream0_only() -> StepExpr<WireProtocolState, WireProtocolAction> {
    nirvash_step_expr! { stream0_only(_prev, action, _next) =>
        stream_for_action(*action) == StreamAtom::Stream0
    }
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
fn one_request_kind_per_stream() -> BoolExpr<WireProtocolState> {
    nirvash_expr! { one_request_kind_per_stream(state) =>
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                RequestKindAtom::bounded_domain()
                    .into_vec()
                    .into_iter()
                    .filter(|kind| state.requests.contains(&stream, kind))
                    .count()
                    <= 1
            })
    }
}

#[invariant(WireProtocolSpec)]
fn logs_flow_requires_ack() -> BoolExpr<WireProtocolState> {
    nirvash_expr! { logs_flow_requires_ack(state) =>
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                (!state.log_follow_streams.contains(&stream)
                    && !state.log_chunks.domain().contains(&stream)
                    && !state.log_ended.contains(&stream))
                    || state.logs_acknowledged(stream)
            })
    }
}

#[invariant(WireProtocolSpec)]
fn command_events_require_command_start() -> BoolExpr<WireProtocolState> {
    nirvash_expr! { command_events_require_command_start(state) =>
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                !state.command_events.domain().contains(&stream)
                    || state.saw_request(stream, RequestKindAtom::CommandStart)
            })
    }
}

#[property(WireProtocolSpec)]
fn logs_request_leads_to_logs_end() -> Ltl<WireProtocolState, WireProtocolAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { logs_acknowledged(state) =>
            state
                .responses
                .range()
                .contains(&RequestKindAtom::LogsRequest)
        }),
        Ltl::pred(nirvash_expr! { logs_follow_active(state) => state.log_follow_streams.some() }),
    )
}

#[fairness(WireProtocolSpec)]
fn logs_progress_fairness() -> Fairness<WireProtocolState, WireProtocolAction> {
    Fairness::weak(nirvash_step_expr! { logs_progress(_prev, action, next) =>
        matches!(
            action,
            WireProtocolAction::LogsChunk(_, _) | WireProtocolAction::LogsEnd(_)
        ) && (next.log_chunks.some() || next.log_ended.some())
    })
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

    fn transition_program(
        &self,
    ) -> Option<::nirvash_core::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule command_event when command_event_payload(action).is_some()
                && prev.saw_request(
                    command_event_stream(action)
                        .expect("command_event guard ensures a stream"),
                    RequestKindAtom::CommandStart,
                ) => {
                set command_events <= command_events_with_event(prev, action);
            }

            rule logs_chunk when logs_chunk_payload(action).is_some()
                && prev.logs_acknowledged(logs_chunk_stream(action)
                    .expect("logs_chunk guard ensures a stream"))
                && !prev.log_ended.contains(&logs_chunk_stream(action)
                    .expect("logs_chunk guard ensures a stream")) => {
                set log_chunks <= log_chunks_with_chunk(prev, action);
            }

            rule logs_end when logs_end_stream(action).is_some()
                && prev.logs_acknowledged(logs_end_stream(action)
                    .expect("logs_end guard ensures a stream"))
                && !prev.log_ended.contains(&logs_end_stream(action)
                    .expect("logs_end guard ensures a stream")) => {
                insert log_ended <= logs_end_stream(action)
                    .expect("logs_end guard ensures a stream");
            }

            rule logs_request_ack when logs_request_stream(action).is_some()
                && !prev.requests.domain().contains(&logs_request_stream(action)
                    .expect("logs_request_ack guard ensures a stream")) => {
                set requests <= request_trace_with_kind(prev, action);
                set responses <= response_trace_with_kind(prev, action);
                insert log_follow_streams <= logs_request_stream(action)
                    .expect("logs_request_ack guard ensures a stream");
            }

            rule request_response when request_response_kind(action).is_some()
                && !prev.requests.domain().contains(&stream_for_action(*action)) => {
                set requests <= request_trace_with_kind(prev, action);
                set responses <= response_trace_with_kind(prev, action);
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = WireProtocolSpec)]
const _: () = ();

fn request_response_kind(action: &WireProtocolAction) -> Option<RequestKindAtom> {
    match action {
        WireProtocolAction::CommandEvent(_, _)
        | WireProtocolAction::LogsChunk(_, _)
        | WireProtocolAction::LogsEnd(_)
        | WireProtocolAction::LogsRequest(_) => None,
        _ => request_kind_for_action(*action),
    }
}

fn logs_request_stream(action: &WireProtocolAction) -> Option<StreamAtom> {
    match action {
        WireProtocolAction::LogsRequest(stream) => Some(*stream),
        _ => None,
    }
}

fn command_event_stream(action: &WireProtocolAction) -> Option<StreamAtom> {
    match action {
        WireProtocolAction::CommandEvent(stream, _) => Some(*stream),
        _ => None,
    }
}

fn command_event_payload(action: &WireProtocolAction) -> Option<CommandEventAtom> {
    match action {
        WireProtocolAction::CommandEvent(_, event) => Some(*event),
        _ => None,
    }
}

fn logs_chunk_stream(action: &WireProtocolAction) -> Option<StreamAtom> {
    match action {
        WireProtocolAction::LogsChunk(stream, _) => Some(*stream),
        _ => None,
    }
}

fn logs_chunk_payload(action: &WireProtocolAction) -> Option<LogChunkAtom> {
    match action {
        WireProtocolAction::LogsChunk(_, chunk) => Some(*chunk),
        _ => None,
    }
}

fn logs_end_stream(action: &WireProtocolAction) -> Option<StreamAtom> {
    match action {
        WireProtocolAction::LogsEnd(stream) => Some(*stream),
        _ => None,
    }
}

fn request_trace_with_kind(
    prev: &WireProtocolState,
    action: &WireProtocolAction,
) -> Relation2<StreamAtom, RequestKindAtom> {
    let kind = request_kind_for_action(*action)
        .expect("request_trace_with_kind requires a request-carrying action");
    let mut requests = prev.requests.clone();
    requests.insert(stream_for_action(*action), kind);
    requests
}

fn response_trace_with_kind(
    prev: &WireProtocolState,
    action: &WireProtocolAction,
) -> Relation2<StreamAtom, RequestKindAtom> {
    let kind = request_kind_for_action(*action)
        .expect("response_trace_with_kind requires a request-carrying action");
    let mut responses = prev.responses.clone();
    responses.insert(stream_for_action(*action), kind);
    responses
}

fn command_events_with_event(
    prev: &WireProtocolState,
    action: &WireProtocolAction,
) -> Relation2<StreamAtom, CommandEventAtom> {
    let stream = command_event_stream(action)
        .expect("command_events_with_event requires CommandEvent action");
    let event = command_event_payload(action)
        .expect("command_events_with_event requires CommandEvent action");
    let mut events = prev.command_events.clone();
    events.insert(stream, event);
    events
}

fn log_chunks_with_chunk(
    prev: &WireProtocolState,
    action: &WireProtocolAction,
) -> Relation2<StreamAtom, LogChunkAtom> {
    let stream = logs_chunk_stream(action).expect("log_chunks_with_chunk requires LogsChunk");
    let chunk = logs_chunk_payload(action).expect("log_chunks_with_chunk requires LogsChunk");
    let mut chunks = prev.log_chunks.clone();
    chunks.insert(stream, chunk);
    chunks
}

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

    #[test]
    fn transition_program_matches_transition_function() {
        let spec = WireProtocolSpec::new();
        let program = spec.transition_program().expect("transition program");
        let initial = spec.initial_state();

        assert_eq!(
            program
                .evaluate(
                    &initial,
                    &WireProtocolAction::LogsRequest(StreamAtom::Stream0)
                )
                .expect("evaluates"),
            transition_wire_protocol(
                &initial,
                &WireProtocolAction::LogsRequest(StreamAtom::Stream0)
            )
        );
        assert_eq!(
            program
                .evaluate(
                    &initial,
                    &WireProtocolAction::CommandEvent(
                        StreamAtom::Stream0,
                        CommandEventAtom::Accepted,
                    ),
                )
                .expect("evaluates"),
            transition_wire_protocol(
                &initial,
                &WireProtocolAction::CommandEvent(StreamAtom::Stream0, CommandEventAtom::Accepted,),
            )
        );
    }
}
