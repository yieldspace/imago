use nirvash::{BoolExpr, Fairness, Ltl, TransitionProgram};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    SymbolicEncoding as FormalSymbolicEncoding, action_constraint, fairness, invariant,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program, property, state_constraint,
    subsystem_spec,
};

use crate::atoms::{RequestKindAtom, SessionAtom, StreamAtom};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum ObservedRole {
    None,
    Admin,
    Client,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum RequestPhase {
    Idle,
    Pending,
    Responded,
    Rejected,
    FollowingLogs0,
    FollowingLogs1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum SessionPhase {
    Inactive,
    ActiveUnknown,
    Admin,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum StreamPhase {
    Closed,
    Pending,
    Responded,
    Rejected,
    FollowingLogs,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub struct ControlPlaneState {
    pub session0: SessionPhase,
    pub session1: SessionPhase,
    pub stream0: StreamPhase,
    pub stream1: StreamPhase,
    pub authority_uploaded: bool,
    pub last_role: ObservedRole,
}

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
pub enum ControlPlaneAction {
    /// Accept a new session.
    AcceptSession(SessionAtom),
    /// Authenticate an admin session.
    AuthenticateAdmin(SessionAtom),
    /// Authenticate a client session.
    AuthenticateClient(SessionAtom),
    /// Mark a session as unknown.
    AuthenticateUnknown(SessionAtom),
    /// Open a request stream.
    OpenRequest(StreamAtom, RequestKindAtom),
    /// Complete a request with a response.
    CompleteResponse(StreamAtom, RequestKindAtom),
    /// Reject a request.
    RejectRequest(StreamAtom, RequestKindAtom),
    /// Start log follow on a stream.
    StartLogFollow(StreamAtom),
    /// Finish log follow on a stream.
    FinishLogFollow(StreamAtom),
    /// Upload a client authority.
    UploadAuthority(SessionAtom),
    /// Close a stream.
    CloseStream(StreamAtom),
    /// Drain one session.
    DrainSession(SessionAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ControlPlaneSpec;

impl ControlPlaneSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> ControlPlaneState {
        ControlPlaneState {
            session0: SessionPhase::Inactive,
            session1: SessionPhase::Inactive,
            stream0: StreamPhase::Closed,
            stream1: StreamPhase::Closed,
            authority_uploaded: false,
            last_role: ObservedRole::None,
        }
    }

    fn representative_request_kind() -> RequestKindAtom {
        RequestKindAtom::ServicesList
    }
}

fn control_plane_model_cases() -> Vec<ModelInstance<ControlPlaneState, ControlPlaneAction>> {
    vec![
        ModelInstance::new("explicit_focus")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::bounded_lasso(4)
            })
            .with_check_deadlocks(false),
        ModelInstance::new("symbolic_focus")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Symbolic),
                bounded_depth: Some(4),
                symbolic: nirvash::SymbolicModelCheckOptions::current()
                    .with_temporal(nirvash::SymbolicTemporalEngine::BoundedLasso),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_check_deadlocks(false),
    ]
}

#[state_constraint(ControlPlaneSpec, cases("explicit_focus", "symbolic_focus"))]
fn symbolic_focus_state() -> BoolExpr<ControlPlaneState> {
    nirvash_expr! { symbolic_focus_state(state) =>
        matches!(state.session1, SessionPhase::Inactive)
            && matches!(state.stream1, StreamPhase::Closed)
    }
}

#[action_constraint(ControlPlaneSpec, cases("explicit_focus", "symbolic_focus"))]
fn symbolic_focus_actions() -> nirvash::StepExpr<ControlPlaneState, ControlPlaneAction> {
    nirvash_step_expr! { symbolic_focus_actions(_prev, action, _next) =>
        matches!(
            action,
            ControlPlaneAction::AcceptSession(SessionAtom::Session0)
                | ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session0)
                | ControlPlaneAction::AuthenticateClient(SessionAtom::Session0)
                | ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session0)
                | ControlPlaneAction::OpenRequest(
                    StreamAtom::Stream0,
                    RequestKindAtom::ServicesList
                )
                | ControlPlaneAction::CompleteResponse(
                    StreamAtom::Stream0,
                    RequestKindAtom::ServicesList
                )
                | ControlPlaneAction::RejectRequest(
                    StreamAtom::Stream0,
                    RequestKindAtom::ServicesList
                )
                | ControlPlaneAction::StartLogFollow(StreamAtom::Stream0)
                | ControlPlaneAction::FinishLogFollow(StreamAtom::Stream0)
                | ControlPlaneAction::UploadAuthority(SessionAtom::Session0)
                | ControlPlaneAction::CloseStream(StreamAtom::Stream0)
                | ControlPlaneAction::DrainSession(SessionAtom::Session0)
        )
    }
}

#[invariant(ControlPlaneSpec)]
fn authority_upload_requires_client_session() -> BoolExpr<ControlPlaneState> {
    nirvash_expr! { authority_upload_requires_client_session(state) =>
        !state.authority_uploaded
            || matches!(state.session0, SessionPhase::Client)
            || matches!(state.session1, SessionPhase::Client)
    }
}

#[invariant(ControlPlaneSpec)]
fn single_stream_lane_is_preserved() -> BoolExpr<ControlPlaneState> {
    nirvash_expr! { single_stream_lane_is_preserved(state) =>
        matches!(state.stream0, StreamPhase::Closed)
            || matches!(state.stream1, StreamPhase::Closed)
    }
}

#[property(ControlPlaneSpec)]
fn authority_upload_preserves_client_role() -> Ltl<ControlPlaneState, ControlPlaneAction> {
    Ltl::always(Ltl::pred(
        nirvash_expr! { authority_upload_preserves_client_role(state) =>
            !state.authority_uploaded || matches!(state.last_role, ObservedRole::Client)
        },
    ))
}

#[property(ControlPlaneSpec)]
fn closed_streams_imply_quiescent_control_plane() -> Ltl<ControlPlaneState, ControlPlaneAction> {
    Ltl::always(Ltl::pred(
        nirvash_expr! { closed_streams_imply_quiescent_control_plane(state) =>
            !(matches!(state.stream0, StreamPhase::Closed)
                && matches!(state.stream1, StreamPhase::Closed))
                || !state.authority_uploaded
                || matches!(state.last_role, ObservedRole::Client)
        },
    ))
}

#[fairness(ControlPlaneSpec)]
fn request_resolution_progress() -> Fairness<ControlPlaneState, ControlPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { request_resolution_progress(prev, action, next) =>
            ((matches!(prev.stream0, StreamPhase::Pending)
                && matches!(
                    action,
                    ControlPlaneAction::CompleteResponse(StreamAtom::Stream0, _)
                        | ControlPlaneAction::RejectRequest(StreamAtom::Stream0, _)
                        | ControlPlaneAction::CloseStream(StreamAtom::Stream0)
                )
                && !matches!(next.stream0, StreamPhase::Pending))
                || (matches!(prev.stream1, StreamPhase::Pending)
                    && matches!(
                        action,
                        ControlPlaneAction::CompleteResponse(StreamAtom::Stream1, _)
                            | ControlPlaneAction::RejectRequest(StreamAtom::Stream1, _)
                            | ControlPlaneAction::CloseStream(StreamAtom::Stream1)
                    )
                    && !matches!(next.stream1, StreamPhase::Pending)))
        },
    )
}

#[fairness(ControlPlaneSpec)]
fn session_drain_progress() -> Fairness<ControlPlaneState, ControlPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { session_drain_progress(prev, action, next) =>
            (matches!(action, ControlPlaneAction::DrainSession(SessionAtom::Session0))
                && !matches!(prev.session0, SessionPhase::Inactive)
                && matches!(next.session0, SessionPhase::Inactive))
                || (matches!(action, ControlPlaneAction::DrainSession(SessionAtom::Session1))
                    && !matches!(prev.session1, SessionPhase::Inactive)
                    && matches!(next.session1, SessionPhase::Inactive))
        },
    )
}

#[subsystem_spec(model_cases(control_plane_model_cases))]
impl FrontendSpec for ControlPlaneSpec {
    type State = ControlPlaneState;
    type Action = ControlPlaneAction;

    fn frontend_name(&self) -> &'static str {
        "control_plane"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        let request_kind = Self::representative_request_kind();
        vec![
            ControlPlaneAction::AcceptSession(SessionAtom::Session0),
            ControlPlaneAction::AcceptSession(SessionAtom::Session1),
            ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session0),
            ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session1),
            ControlPlaneAction::AuthenticateClient(SessionAtom::Session0),
            ControlPlaneAction::AuthenticateClient(SessionAtom::Session1),
            ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session0),
            ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session1),
            ControlPlaneAction::OpenRequest(StreamAtom::Stream0, request_kind),
            ControlPlaneAction::OpenRequest(StreamAtom::Stream1, request_kind),
            ControlPlaneAction::CompleteResponse(StreamAtom::Stream0, request_kind),
            ControlPlaneAction::CompleteResponse(StreamAtom::Stream1, request_kind),
            ControlPlaneAction::RejectRequest(StreamAtom::Stream0, request_kind),
            ControlPlaneAction::RejectRequest(StreamAtom::Stream1, request_kind),
            ControlPlaneAction::StartLogFollow(StreamAtom::Stream0),
            ControlPlaneAction::StartLogFollow(StreamAtom::Stream1),
            ControlPlaneAction::FinishLogFollow(StreamAtom::Stream0),
            ControlPlaneAction::FinishLogFollow(StreamAtom::Stream1),
            ControlPlaneAction::UploadAuthority(SessionAtom::Session0),
            ControlPlaneAction::UploadAuthority(SessionAtom::Session1),
            ControlPlaneAction::CloseStream(StreamAtom::Stream0),
            ControlPlaneAction::CloseStream(StreamAtom::Stream1),
            ControlPlaneAction::DrainSession(SessionAtom::Session0),
            ControlPlaneAction::DrainSession(SessionAtom::Session1),
        ]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule accept_session_0 when matches!(action, ControlPlaneAction::AcceptSession(SessionAtom::Session0))
                && matches!(prev.session0, SessionPhase::Inactive) => {
                set session0 <= SessionPhase::ActiveUnknown;
                set authority_uploaded <= false;
            }

            rule accept_session_1 when matches!(action, ControlPlaneAction::AcceptSession(SessionAtom::Session1))
                && matches!(prev.session1, SessionPhase::Inactive) => {
                set session1 <= SessionPhase::ActiveUnknown;
                set authority_uploaded <= false;
            }

            rule authenticate_admin_0 when matches!(action, ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session0))
                && !matches!(prev.session0, SessionPhase::Inactive) => {
                set session0 <= SessionPhase::Admin;
                set last_role <= ObservedRole::Admin;
                set authority_uploaded <= false;
            }

            rule authenticate_admin_1 when matches!(action, ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session1))
                && !matches!(prev.session1, SessionPhase::Inactive) => {
                set session1 <= SessionPhase::Admin;
                set last_role <= ObservedRole::Admin;
                set authority_uploaded <= false;
            }

            rule authenticate_client_0 when matches!(action, ControlPlaneAction::AuthenticateClient(SessionAtom::Session0))
                && !matches!(prev.session0, SessionPhase::Inactive) => {
                set session0 <= SessionPhase::Client;
                set last_role <= ObservedRole::Client;
            }

            rule authenticate_client_1 when matches!(action, ControlPlaneAction::AuthenticateClient(SessionAtom::Session1))
                && !matches!(prev.session1, SessionPhase::Inactive) => {
                set session1 <= SessionPhase::Client;
                set last_role <= ObservedRole::Client;
            }

            rule authenticate_unknown_0 when matches!(action, ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session0))
                && !matches!(prev.session0, SessionPhase::Inactive) => {
                set session0 <= SessionPhase::ActiveUnknown;
                set last_role <= ObservedRole::Unknown;
                set authority_uploaded <= false;
            }

            rule authenticate_unknown_1 when matches!(action, ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session1))
                && !matches!(prev.session1, SessionPhase::Inactive) => {
                set session1 <= SessionPhase::ActiveUnknown;
                set last_role <= ObservedRole::Unknown;
                set authority_uploaded <= false;
            }

            rule open_request_0 when matches!(action, ControlPlaneAction::OpenRequest(StreamAtom::Stream0, _))
                && matches!(prev.stream0, StreamPhase::Closed)
                && matches!(prev.stream1, StreamPhase::Closed) => {
                set stream0 <= StreamPhase::Pending;
            }

            rule open_request_1 when matches!(action, ControlPlaneAction::OpenRequest(StreamAtom::Stream1, _))
                && matches!(prev.stream0, StreamPhase::Closed)
                && matches!(prev.stream1, StreamPhase::Closed) => {
                set stream1 <= StreamPhase::Pending;
            }

            rule complete_response_0 when matches!(action, ControlPlaneAction::CompleteResponse(StreamAtom::Stream0, _))
                && matches!(prev.stream0, StreamPhase::Pending) => {
                set stream0 <= StreamPhase::Responded;
            }

            rule complete_response_1 when matches!(action, ControlPlaneAction::CompleteResponse(StreamAtom::Stream1, _))
                && matches!(prev.stream1, StreamPhase::Pending) => {
                set stream1 <= StreamPhase::Responded;
            }

            rule reject_request_0 when matches!(action, ControlPlaneAction::RejectRequest(StreamAtom::Stream0, _))
                && matches!(prev.stream0, StreamPhase::Pending) => {
                set stream0 <= StreamPhase::Rejected;
            }

            rule reject_request_1 when matches!(action, ControlPlaneAction::RejectRequest(StreamAtom::Stream1, _))
                && matches!(prev.stream1, StreamPhase::Pending) => {
                set stream1 <= StreamPhase::Rejected;
            }

            rule start_log_follow_0 when matches!(action, ControlPlaneAction::StartLogFollow(StreamAtom::Stream0))
                && !matches!(prev.stream0, StreamPhase::Closed) => {
                set stream0 <= StreamPhase::FollowingLogs;
            }

            rule start_log_follow_1 when matches!(action, ControlPlaneAction::StartLogFollow(StreamAtom::Stream1))
                && !matches!(prev.stream1, StreamPhase::Closed) => {
                set stream1 <= StreamPhase::FollowingLogs;
            }

            rule finish_log_follow_0 when matches!(action, ControlPlaneAction::FinishLogFollow(StreamAtom::Stream0))
                && matches!(prev.stream0, StreamPhase::FollowingLogs) => {
                set stream0 <= StreamPhase::Closed;
            }

            rule finish_log_follow_1 when matches!(action, ControlPlaneAction::FinishLogFollow(StreamAtom::Stream1))
                && matches!(prev.stream1, StreamPhase::FollowingLogs) => {
                set stream1 <= StreamPhase::Closed;
            }

            rule upload_authority_0 when matches!(action, ControlPlaneAction::UploadAuthority(SessionAtom::Session0))
                && matches!(prev.session0, SessionPhase::Client) => {
                set authority_uploaded <= true;
            }

            rule upload_authority_1 when matches!(action, ControlPlaneAction::UploadAuthority(SessionAtom::Session1))
                && matches!(prev.session1, SessionPhase::Client) => {
                set authority_uploaded <= true;
            }

            rule close_stream_0 when matches!(action, ControlPlaneAction::CloseStream(StreamAtom::Stream0))
                && !matches!(prev.stream0, StreamPhase::Closed) => {
                set stream0 <= StreamPhase::Closed;
            }

            rule close_stream_1 when matches!(action, ControlPlaneAction::CloseStream(StreamAtom::Stream1))
                && !matches!(prev.stream1, StreamPhase::Closed) => {
                set stream1 <= StreamPhase::Closed;
            }

            rule drain_session_0 when matches!(action, ControlPlaneAction::DrainSession(SessionAtom::Session0))
                && !matches!(prev.session0, SessionPhase::Inactive) => {
                set session0 <= SessionPhase::Inactive;
                set authority_uploaded <= if matches!(prev.session1, SessionPhase::Client) {
                    true
                } else {
                    false
                };
            }

            rule drain_session_1 when matches!(action, ControlPlaneAction::DrainSession(SessionAtom::Session1))
                && !matches!(prev.session1, SessionPhase::Inactive) => {
                set session1 <= SessionPhase::Inactive;
                set authority_uploaded <= if matches!(prev.session0, SessionPhase::Client) {
                    true
                } else {
                    false
                };
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_check as checks;

    fn case_by_label(
        spec: &ControlPlaneSpec,
        label: &str,
    ) -> nirvash_lower::ModelInstance<ControlPlaneState, ControlPlaneAction> {
        spec.model_instances()
            .into_iter()
            .find(|case| case.label() == label)
            .expect("model case should exist")
    }

    fn bounded_parity_case(
        case: nirvash_lower::ModelInstance<ControlPlaneState, ControlPlaneAction>,
    ) -> nirvash_lower::ModelInstance<ControlPlaneState, ControlPlaneAction> {
        let mut config = case.effective_checker_config();
        let doc_config = case.doc_checker_config().map(|mut config| {
            config.max_states = Some(64);
            config.max_transitions = Some(256);
            config
        });
        config.max_states = Some(64);
        config.max_transitions = Some(256);
        let case = case.with_checker_config(config);
        match doc_config {
            Some(doc_config) => case.with_doc_checker_config(doc_config),
            None => case,
        }
    }

    #[test]
    fn explicit_and_symbolic_backends_agree() {
        let spec = ControlPlaneSpec::new();
        let lowered = crate::lowered_spec(&spec);
        let explicit_case = bounded_parity_case(case_by_label(&spec, "explicit_focus"));
        let symbolic_case = bounded_parity_case(case_by_label(&spec, "symbolic_focus"));

        let explicit_snapshot =
            checks::ExplicitModelChecker::for_case(&lowered, explicit_case.clone())
                .reachable_graph_snapshot()
                .expect("explicit control snapshot");
        let symbolic_snapshot =
            checks::SymbolicModelChecker::for_case(&lowered, symbolic_case.clone())
                .reachable_graph_snapshot()
                .expect("symbolic control snapshot");
        assert_eq!(symbolic_snapshot, explicit_snapshot);
    }
}
