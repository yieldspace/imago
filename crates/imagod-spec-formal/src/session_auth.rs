use nirvash_core::{
    ActionConstraint, Fairness, Ltl, ModelCase, ModelCheckConfig, RelSet, Relation2,
    Signature as _, StatePredicate, StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelationalState, action_constraint, fairness, invariant, property,
    subsystem_spec,
};

use crate::atoms::{
    RemoteAuthorityAtom, RequestKindAtom, SessionAtom, SessionRoleAtom, StreamAtom,
};

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
pub struct SessionAuthState {
    accepted_sessions: RelSet<SessionAtom>,
    authenticated_roles: Relation2<SessionAtom, SessionRoleAtom>,
    admin_authorized_streams: Relation2<StreamAtom, RequestKindAtom>,
    client_authorized_streams: Relation2<StreamAtom, RequestKindAtom>,
    rejected_streams: Relation2<StreamAtom, RequestKindAtom>,
    timed_out_streams: RelSet<StreamAtom>,
    closed_streams: RelSet<StreamAtom>,
    uploaded_authorities: RelSet<RemoteAuthorityAtom>,
}

impl SessionAuthState {
    pub fn authority_uploaded(&self, authority: RemoteAuthorityAtom) -> bool {
        self.uploaded_authorities.contains(&authority)
    }

    pub fn session_accepted(&self, session: SessionAtom) -> bool {
        self.accepted_sessions.contains(&session)
    }

    pub fn session_authenticated(&self, session: SessionAtom) -> bool {
        self.authenticated_roles.domain().contains(&session)
    }

    pub fn any_authenticated_as(&self, role: SessionRoleAtom) -> bool {
        self.authenticated_roles.range().contains(&role)
    }

    pub fn stream_authorized(&self, stream: StreamAtom, kind: RequestKindAtom) -> bool {
        self.admin_authorized_streams.contains(&stream, &kind)
            || self.client_authorized_streams.contains(&stream, &kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, nirvash_macros::Signature, ActionVocabulary)]
pub enum SessionAuthAction {
    /// Accept a new session.
    AcceptSession(SessionAtom),
    /// Authenticate one accepted session as admin.
    AuthenticateAdmin(SessionAtom),
    /// Authenticate one accepted session as client.
    AuthenticateClient(SessionAtom),
    /// Authenticate one accepted session as unknown.
    AuthenticateUnknown(SessionAtom),
    /// Authorize one stream for an admin session.
    AuthorizeAdmin(StreamAtom, RequestKindAtom),
    /// Authorize one stream for a client session.
    AuthorizeClient(StreamAtom, RequestKindAtom),
    /// Reject one unauthorized stream.
    RejectUnauthorized(StreamAtom, RequestKindAtom),
    /// Time out one open stream.
    ReadTimeout(StreamAtom),
    /// Close one stream.
    CloseStream(StreamAtom),
    /// Register one dynamically uploaded client authority.
    UploadClientAuthority(RemoteAuthorityAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SessionAuthSpec;

impl SessionAuthSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> SessionAuthState {
        SessionAuthState {
            accepted_sessions: RelSet::empty(),
            authenticated_roles: Relation2::empty(),
            admin_authorized_streams: Relation2::empty(),
            client_authorized_streams: Relation2::empty(),
            rejected_streams: Relation2::empty(),
            timed_out_streams: RelSet::empty(),
            closed_streams: RelSet::empty(),
            uploaded_authorities: RelSet::empty(),
        }
    }
}

fn session_auth_model_cases() -> Vec<ModelCase<SessionAuthState, SessionAuthAction>> {
    vec![
        ModelCase::default()
            .with_label("client_rpc_surface")
            .with_checker_config(ModelCheckConfig {
                exploration: nirvash_core::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(1024),
                max_transitions: Some(3072),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_doc_checker_config(ModelCheckConfig {
                exploration: nirvash_core::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(128),
                max_transitions: Some(384),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_check_deadlocks(false),
        ModelCase::new("timeout_and_reject_surface")
            .with_checker_config(ModelCheckConfig {
                exploration: nirvash_core::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(1024),
                max_transitions: Some(3072),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_doc_checker_config(ModelCheckConfig {
                exploration: nirvash_core::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(128),
                max_transitions: Some(384),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_check_deadlocks(false),
    ]
}

#[action_constraint(SessionAuthSpec, cases("client_rpc_surface"))]
fn client_rpc_surface_actions() -> ActionConstraint<SessionAuthState, SessionAuthAction> {
    ActionConstraint::new("client_rpc_surface_actions", |_, action, _| {
        matches!(
            action,
            SessionAuthAction::AcceptSession(SessionAtom::Session0)
                | SessionAuthAction::AuthenticateClient(SessionAtom::Session0)
                | SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::HelloNegotiate,
                )
                | SessionAuthAction::UploadClientAuthority(RemoteAuthorityAtom::Edge0)
                | SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::RpcInvoke,
                )
        )
    })
}

#[action_constraint(SessionAuthSpec, cases("timeout_and_reject_surface"))]
fn session0_only() -> ActionConstraint<SessionAuthState, SessionAuthAction> {
    ActionConstraint::new("session0_only", |_, action, _| {
        session_for_action(*action).is_none_or(|session| session == SessionAtom::Session0)
    })
}

#[action_constraint(SessionAuthSpec, cases("timeout_and_reject_surface"))]
fn timeout_reject_surface() -> ActionConstraint<SessionAuthState, SessionAuthAction> {
    ActionConstraint::new("timeout_reject_surface", |_, action, _| match action {
        SessionAuthAction::AuthorizeAdmin(stream, kind)
        | SessionAuthAction::RejectUnauthorized(stream, kind) => {
            *stream == StreamAtom::Stream0 && *kind == RequestKindAtom::ServicesList
        }
        SessionAuthAction::ReadTimeout(stream) | SessionAuthAction::CloseStream(stream) => {
            *stream == StreamAtom::Stream0
        }
        SessionAuthAction::AcceptSession(SessionAtom::Session0)
        | SessionAuthAction::AuthenticateAdmin(SessionAtom::Session0) => true,
        _ => false,
    })
}

fn session_for_action(action: SessionAuthAction) -> Option<SessionAtom> {
    match action {
        SessionAuthAction::AcceptSession(session)
        | SessionAuthAction::AuthenticateAdmin(session)
        | SessionAuthAction::AuthenticateClient(session)
        | SessionAuthAction::AuthenticateUnknown(session) => Some(session),
        SessionAuthAction::AuthorizeAdmin(_, _)
        | SessionAuthAction::AuthorizeClient(_, _)
        | SessionAuthAction::RejectUnauthorized(_, _)
        | SessionAuthAction::ReadTimeout(_)
        | SessionAuthAction::CloseStream(_)
        | SessionAuthAction::UploadClientAuthority(_) => None,
    }
}

fn client_request_allowed(kind: RequestKindAtom) -> bool {
    matches!(
        kind,
        RequestKindAtom::HelloNegotiate | RequestKindAtom::RpcInvoke
    )
}

#[invariant(SessionAuthSpec)]
fn authorization_excludes_closed_or_timed_out_streams() -> StatePredicate<SessionAuthState> {
    StatePredicate::new(
        "authorization_excludes_closed_or_timed_out_streams",
        |state| {
            StreamAtom::bounded_domain()
                .into_vec()
                .into_iter()
                .all(|stream| {
                    (!state.timed_out_streams.contains(&stream)
                        && !state.closed_streams.contains(&stream))
                        || (!state.admin_authorized_streams.domain().contains(&stream)
                            && !state.client_authorized_streams.domain().contains(&stream))
                })
        },
    )
}

#[invariant(SessionAuthSpec)]
fn client_authorization_is_limited() -> StatePredicate<SessionAuthState> {
    StatePredicate::new("client_authorization_is_limited", |state| {
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                RequestKindAtom::bounded_domain()
                    .into_vec()
                    .into_iter()
                    .all(|kind| {
                        !state.client_authorized_streams.contains(&stream, &kind)
                            || client_request_allowed(kind)
                    })
            })
    })
}

#[property(SessionAuthSpec)]
fn accepted_session_leads_to_authentication() -> Ltl<SessionAuthState, SessionAuthAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("accepted_session", |state| {
            state.accepted_sessions.some()
        })),
        Ltl::pred(StatePredicate::new("authenticated_session", |state| {
            state.authenticated_roles.some()
        })),
    )
}

#[fairness(SessionAuthSpec)]
fn authentication_progress_fairness() -> Fairness<SessionAuthState, SessionAuthAction> {
    Fairness::weak(StepPredicate::new(
        "authenticate_session",
        |_, action, next| {
            matches!(
                action,
                SessionAuthAction::AuthenticateAdmin(_)
                    | SessionAuthAction::AuthenticateClient(_)
                    | SessionAuthAction::AuthenticateUnknown(_)
            ) && next.authenticated_roles.some()
        },
    ))
}

#[subsystem_spec(model_cases(session_auth_model_cases))]
impl TransitionSystem for SessionAuthSpec {
    type State = SessionAuthState;
    type Action = SessionAuthAction;

    fn name(&self) -> &'static str {
        "session_auth"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_session_auth(state, action)
    }
}

#[nirvash_macros::formal_tests(spec = SessionAuthSpec)]
const _: () = ();

fn transition_session_auth(
    prev: &SessionAuthState,
    action: &SessionAuthAction,
) -> Option<SessionAuthState> {
    let mut candidate = prev.clone();
    let allowed = match action {
        SessionAuthAction::AcceptSession(session) if !prev.accepted_sessions.contains(session) => {
            candidate.accepted_sessions.insert(*session);
            true
        }
        SessionAuthAction::AuthenticateAdmin(session)
            if prev.accepted_sessions.contains(session)
                && !prev.authenticated_roles.domain().contains(session) =>
        {
            candidate
                .authenticated_roles
                .insert(*session, SessionRoleAtom::Admin);
            true
        }
        SessionAuthAction::AuthenticateClient(session)
            if prev.accepted_sessions.contains(session)
                && !prev.authenticated_roles.domain().contains(session) =>
        {
            candidate
                .authenticated_roles
                .insert(*session, SessionRoleAtom::Client);
            true
        }
        SessionAuthAction::AuthenticateUnknown(session)
            if prev.accepted_sessions.contains(session)
                && !prev.authenticated_roles.domain().contains(session) =>
        {
            candidate
                .authenticated_roles
                .insert(*session, SessionRoleAtom::Unknown);
            true
        }
        SessionAuthAction::AuthorizeAdmin(stream, kind)
            if prev
                .authenticated_roles
                .range()
                .contains(&SessionRoleAtom::Admin)
                && !prev.timed_out_streams.contains(stream)
                && !prev.closed_streams.contains(stream) =>
        {
            candidate.admin_authorized_streams.insert(*stream, *kind);
            true
        }
        SessionAuthAction::AuthorizeClient(stream, kind)
            if prev
                .authenticated_roles
                .range()
                .contains(&SessionRoleAtom::Client)
                && client_request_allowed(*kind)
                && !prev.timed_out_streams.contains(stream)
                && !prev.closed_streams.contains(stream) =>
        {
            candidate.client_authorized_streams.insert(*stream, *kind);
            true
        }
        SessionAuthAction::RejectUnauthorized(stream, kind)
            if !prev.stream_authorized(*stream, *kind) =>
        {
            candidate.rejected_streams.insert(*stream, *kind);
            true
        }
        SessionAuthAction::ReadTimeout(stream)
            if !prev.timed_out_streams.contains(stream)
                && !prev.closed_streams.contains(stream) =>
        {
            candidate.timed_out_streams.insert(*stream);
            for kind in RequestKindAtom::bounded_domain().into_vec() {
                candidate.admin_authorized_streams.remove(stream, &kind);
                candidate.client_authorized_streams.remove(stream, &kind);
            }
            true
        }
        SessionAuthAction::CloseStream(stream) if !prev.closed_streams.contains(stream) => {
            candidate.closed_streams.insert(*stream);
            for kind in RequestKindAtom::bounded_domain().into_vec() {
                candidate.admin_authorized_streams.remove(stream, &kind);
                candidate.client_authorized_streams.remove(stream, &kind);
            }
            true
        }
        SessionAuthAction::UploadClientAuthority(authority)
            if !prev.uploaded_authorities.contains(authority) =>
        {
            candidate.uploaded_authorities.insert(*authority);
            true
        }
        _ => false,
    };

    allowed.then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_authorization_is_limited_to_hello_and_rpc() {
        let spec = SessionAuthSpec::new();
        let state = spec
            .transition(
                &spec.initial_state(),
                &SessionAuthAction::AcceptSession(SessionAtom::Session0),
            )
            .and_then(|state| {
                spec.transition(
                    &state,
                    &SessionAuthAction::AuthenticateClient(SessionAtom::Session0),
                )
            })
            .expect("client authentication should succeed");

        assert!(
            spec.transition(
                &state,
                &SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::RpcInvoke
                ),
            )
            .is_some()
        );
        assert!(
            spec.transition(
                &state,
                &SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::ServicesList,
                ),
            )
            .is_none()
        );
    }
}
