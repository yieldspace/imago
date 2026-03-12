use nirvash::{
    BoolExpr, Fairness, Ltl, ModelBackend, ModelCase, ModelCheckConfig, RelSet, Relation2,
    StepExpr, TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelationalState, Signature as FormalSignature, action_constraint, fairness,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program, property, subsystem_spec,
};

use crate::atoms::{
    RemoteAuthorityAtom, RequestKindAtom, SessionAtom, SessionRoleAtom, StreamAtom,
};
use crate::summary_mapping::session_role_atom;

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature, RelationalState)]
#[signature(custom)]
pub struct SessionAuthState {
    accepted_sessions: RelSet<SessionAtom>,
    authenticated_roles: Relation2<SessionAtom, SessionRoleAtom>,
    timed_out_streams: RelSet<StreamAtom>,
    closed_streams: RelSet<StreamAtom>,
    uploaded_authorities: RelSet<RemoteAuthorityAtom>,
}

impl SessionAuthState {
    pub fn from_router_summary(summary: &imagod_spec::RouterStateSummary) -> Self {
        let mut state = Self {
            accepted_sessions: RelSet::empty(),
            authenticated_roles: Relation2::empty(),
            timed_out_streams: RelSet::empty(),
            closed_streams: RelSet::empty(),
            uploaded_authorities: RelSet::empty(),
        };
        if summary.active_session {
            state.accepted_sessions.insert(SessionAtom::Session0);
        }
        if let Some(role) = summary.role {
            state
                .authenticated_roles
                .insert(SessionAtom::Session0, session_role_atom(role));
        }
        if summary.authority_uploaded {
            state
                .uploaded_authorities
                .insert(RemoteAuthorityAtom::Edge0);
        }
        state
    }

    pub fn from_summary(summary: &imagod_spec::SessionAuthStateSummary) -> Self {
        let mut state = Self {
            accepted_sessions: RelSet::empty(),
            authenticated_roles: Relation2::empty(),
            timed_out_streams: RelSet::empty(),
            closed_streams: RelSet::empty(),
            uploaded_authorities: RelSet::empty(),
        };
        if summary.active_session {
            state.accepted_sessions.insert(SessionAtom::Session0);
        }
        if let Some(role) = summary.role {
            state
                .authenticated_roles
                .insert(SessionAtom::Session0, session_role_atom(role));
        }
        if summary.read_timed_out {
            state.timed_out_streams.insert(StreamAtom::Stream0);
        }
        if summary.stream_closed {
            state.closed_streams.insert(StreamAtom::Stream0);
        }
        if summary.client_authority_uploaded {
            state
                .uploaded_authorities
                .insert(RemoteAuthorityAtom::Edge0);
        }
        state
    }

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
        if self.timed_out_streams.contains(&stream) || self.closed_streams.contains(&stream) {
            return false;
        }

        if self.any_authenticated_as(SessionRoleAtom::Admin) {
            return matches!(
                kind,
                RequestKindAtom::DeployPrepare
                    | RequestKindAtom::ArtifactPush
                    | RequestKindAtom::ArtifactCommit
                    | RequestKindAtom::CommandStart
                    | RequestKindAtom::StateRequest
                    | RequestKindAtom::ServicesList
                    | RequestKindAtom::CommandCancel
                    | RequestKindAtom::RpcInvoke
                    | RequestKindAtom::BindingsCertUpload
                    | RequestKindAtom::LogsRequest
            );
        }

        self.any_authenticated_as(SessionRoleAtom::Client)
            && match kind {
                RequestKindAtom::HelloNegotiate => true,
                RequestKindAtom::RpcInvoke => self.authority_uploaded(RemoteAuthorityAtom::Edge0),
                _ => false,
            }
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
            timed_out_streams: RelSet::empty(),
            closed_streams: RelSet::empty(),
            uploaded_authorities: RelSet::empty(),
        }
    }
}

nirvash::signature_spec!(
    SessionAuthStateSignatureSpec for SessionAuthState,
    representatives = crate::state_domain::reachable_state_domain(&SessionAuthSpec::new())
);

nirvash::symbolic_state_spec!(for SessionAuthState {
    accepted_sessions: RelSet<SessionAtom>,
    authenticated_roles: Relation2<SessionAtom, SessionRoleAtom>,
    timed_out_streams: RelSet<StreamAtom>,
    closed_streams: RelSet<StreamAtom>,
    uploaded_authorities: RelSet<RemoteAuthorityAtom>,
});

fn session_auth_model_cases() -> Vec<ModelCase<SessionAuthState, SessionAuthAction>> {
    vec![
        ModelCase::default()
            .with_label("client_rpc_surface")
            .with_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                exploration: nirvash::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(1024),
                max_transitions: Some(3072),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_doc_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                exploration: nirvash::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(128),
                max_transitions: Some(384),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_check_deadlocks(false),
        ModelCase::new("timeout_and_reject_surface")
            .with_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                exploration: nirvash::ExplorationMode::ReachableGraph,
                bounded_depth: None,
                max_states: Some(1024),
                max_transitions: Some(3072),
                check_deadlocks: false,
                stop_on_first_violation: false,
            })
            .with_doc_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                exploration: nirvash::ExplorationMode::ReachableGraph,
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
fn client_rpc_surface_actions() -> StepExpr<SessionAuthState, SessionAuthAction> {
    nirvash_step_expr! { client_rpc_surface_actions(_prev, action, _next) =>
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
    }
}

#[action_constraint(SessionAuthSpec, cases("timeout_and_reject_surface"))]
fn session0_only() -> StepExpr<SessionAuthState, SessionAuthAction> {
    nirvash_step_expr! { session0_only(_prev, action, _next) =>
        session_for_action(*action).is_none_or(|session| session == SessionAtom::Session0)
    }
}

#[action_constraint(SessionAuthSpec, cases("timeout_and_reject_surface"))]
fn timeout_reject_surface() -> StepExpr<SessionAuthState, SessionAuthAction> {
    nirvash_step_expr! { timeout_reject_surface(_prev, action, _next) =>
        timeout_reject_surface_action(*action)
    }
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

#[property(SessionAuthSpec)]
fn accepted_session_leads_to_authentication() -> Ltl<SessionAuthState, SessionAuthAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { accepted_session(state) => state.accepted_sessions.some() }),
        Ltl::pred(nirvash_expr! { authenticated_session(state) =>
            state.authenticated_roles.some()
        }),
    )
}

#[fairness(SessionAuthSpec)]
fn authentication_progress_fairness() -> Fairness<SessionAuthState, SessionAuthAction> {
    Fairness::weak(
        nirvash_step_expr! { authenticate_session(_prev, action, next) =>
            matches!(
                action,
                SessionAuthAction::AuthenticateAdmin(_)
                    | SessionAuthAction::AuthenticateClient(_)
                    | SessionAuthAction::AuthenticateUnknown(_)
            ) && next.authenticated_roles.some()
        },
    )
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
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule accept_session when accept_session_from_action(action).is_some()
                && !prev.accepted_sessions.contains(&accept_session_from_action(action)
                    .expect("accept_session guard ensures a session")) => {
                insert accepted_sessions <= accept_session_from_action(action)
                    .expect("accept_session guard ensures a session");
            }

            rule authenticate_admin when authenticate_admin_session(action).is_some()
                && prev.accepted_sessions.contains(&authenticate_admin_session(action)
                    .expect("authenticate_admin guard ensures a session"))
                && !prev.authenticated_roles.domain().contains(&authenticate_admin_session(action)
                    .expect("authenticate_admin guard ensures a session")) => {
                set authenticated_roles <= authenticate_admin_roles(prev, action);
            }

            rule authenticate_client when authenticate_client_session(action).is_some()
                && prev.accepted_sessions.contains(&authenticate_client_session(action)
                    .expect("authenticate_client guard ensures a session"))
                && !prev.authenticated_roles.domain().contains(&authenticate_client_session(action)
                    .expect("authenticate_client guard ensures a session")) => {
                set authenticated_roles <= authenticate_client_roles(prev, action);
            }

            rule authenticate_unknown when authenticate_unknown_session(action).is_some()
                && prev.accepted_sessions.contains(&authenticate_unknown_session(action)
                    .expect("authenticate_unknown guard ensures a session"))
                && !prev.authenticated_roles.domain().contains(&authenticate_unknown_session(action)
                    .expect("authenticate_unknown guard ensures a session")) => {
                set authenticated_roles <= authenticate_unknown_roles(prev, action);
            }

            rule authorize_admin when authorize_admin_allowed(prev, action) => {
            }

            rule authorize_client when authorize_client_allowed(prev, action) => {
            }

            rule reject_unauthorized when reject_unauthorized_required(prev, action) => {
            }

            rule read_timeout when read_timeout_stream(action).is_some()
                && !prev.timed_out_streams.contains(&read_timeout_stream(action)
                    .expect("read_timeout guard ensures a stream"))
                && !prev.closed_streams.contains(&read_timeout_stream(action)
                    .expect("read_timeout guard ensures a stream")) => {
                insert timed_out_streams <= read_timeout_stream(action)
                    .expect("read_timeout guard ensures a stream");
            }

            rule close_stream when close_stream_from_action(action).is_some()
                && !prev.closed_streams.contains(&close_stream_from_action(action)
                    .expect("close_stream guard ensures a stream")) => {
                insert closed_streams <= close_stream_from_action(action)
                    .expect("close_stream guard ensures a stream");
            }

            rule upload_client_authority when upload_client_authority_from_action(action).is_some()
                && !prev.uploaded_authorities.contains(&upload_client_authority_from_action(action)
                    .expect("upload_client_authority guard ensures an authority")) => {
                insert uploaded_authorities <= upload_client_authority_from_action(action)
                    .expect("upload_client_authority guard ensures an authority");
            }
        })
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
            if prev.stream_authorized(*stream, *kind) =>
        {
            true
        }
        SessionAuthAction::AuthorizeClient(stream, kind)
            if prev.stream_authorized(*stream, *kind) =>
        {
            true
        }
        SessionAuthAction::RejectUnauthorized(stream, kind)
            if !prev.stream_authorized(*stream, *kind) =>
        {
            true
        }
        SessionAuthAction::ReadTimeout(stream)
            if !prev.timed_out_streams.contains(stream)
                && !prev.closed_streams.contains(stream) =>
        {
            candidate.timed_out_streams.insert(*stream);
            true
        }
        SessionAuthAction::CloseStream(stream) if !prev.closed_streams.contains(stream) => {
            candidate.closed_streams.insert(*stream);
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

fn timeout_reject_surface_action(action: SessionAuthAction) -> bool {
    match action {
        SessionAuthAction::AuthorizeAdmin(stream, kind)
        | SessionAuthAction::RejectUnauthorized(stream, kind) => {
            stream == StreamAtom::Stream0 && kind == RequestKindAtom::ServicesList
        }
        SessionAuthAction::ReadTimeout(stream) | SessionAuthAction::CloseStream(stream) => {
            stream == StreamAtom::Stream0
        }
        SessionAuthAction::AcceptSession(SessionAtom::Session0)
        | SessionAuthAction::AuthenticateAdmin(SessionAtom::Session0) => true,
        _ => false,
    }
}

fn accept_session_from_action(action: &SessionAuthAction) -> Option<SessionAtom> {
    match action {
        SessionAuthAction::AcceptSession(session) => Some(*session),
        _ => None,
    }
}

fn authenticate_admin_session(action: &SessionAuthAction) -> Option<SessionAtom> {
    match action {
        SessionAuthAction::AuthenticateAdmin(session) => Some(*session),
        _ => None,
    }
}

fn authenticate_client_session(action: &SessionAuthAction) -> Option<SessionAtom> {
    match action {
        SessionAuthAction::AuthenticateClient(session) => Some(*session),
        _ => None,
    }
}

fn authenticate_unknown_session(action: &SessionAuthAction) -> Option<SessionAtom> {
    match action {
        SessionAuthAction::AuthenticateUnknown(session) => Some(*session),
        _ => None,
    }
}

fn read_timeout_stream(action: &SessionAuthAction) -> Option<StreamAtom> {
    match action {
        SessionAuthAction::ReadTimeout(stream) => Some(*stream),
        _ => None,
    }
}

fn close_stream_from_action(action: &SessionAuthAction) -> Option<StreamAtom> {
    match action {
        SessionAuthAction::CloseStream(stream) => Some(*stream),
        _ => None,
    }
}

fn upload_client_authority_from_action(action: &SessionAuthAction) -> Option<RemoteAuthorityAtom> {
    match action {
        SessionAuthAction::UploadClientAuthority(authority) => Some(*authority),
        _ => None,
    }
}

fn authorize_admin_allowed(prev: &SessionAuthState, action: &SessionAuthAction) -> bool {
    match action {
        SessionAuthAction::AuthorizeAdmin(stream, kind) => prev.stream_authorized(*stream, *kind),
        _ => false,
    }
}

fn authorize_client_allowed(prev: &SessionAuthState, action: &SessionAuthAction) -> bool {
    match action {
        SessionAuthAction::AuthorizeClient(stream, kind) => prev.stream_authorized(*stream, *kind),
        _ => false,
    }
}

fn reject_unauthorized_required(prev: &SessionAuthState, action: &SessionAuthAction) -> bool {
    match action {
        SessionAuthAction::RejectUnauthorized(stream, kind) => {
            !prev.stream_authorized(*stream, *kind)
        }
        _ => false,
    }
}

fn authenticated_roles_with(
    prev: &SessionAuthState,
    session: SessionAtom,
    role: SessionRoleAtom,
) -> Relation2<SessionAtom, SessionRoleAtom> {
    let mut roles = prev.authenticated_roles.clone();
    roles.insert(session, role);
    roles
}

fn authenticate_admin_roles(
    prev: &SessionAuthState,
    action: &SessionAuthAction,
) -> Relation2<SessionAtom, SessionRoleAtom> {
    let session = authenticate_admin_session(action)
        .expect("authenticate_admin_roles requires AuthenticateAdmin action");
    authenticated_roles_with(prev, session, SessionRoleAtom::Admin)
}

fn authenticate_client_roles(
    prev: &SessionAuthState,
    action: &SessionAuthAction,
) -> Relation2<SessionAtom, SessionRoleAtom> {
    let session = authenticate_client_session(action)
        .expect("authenticate_client_roles requires AuthenticateClient action");
    authenticated_roles_with(prev, session, SessionRoleAtom::Client)
}

fn authenticate_unknown_roles(
    prev: &SessionAuthState,
    action: &SessionAuthAction,
) -> Relation2<SessionAtom, SessionRoleAtom> {
    let session = authenticate_unknown_session(action)
        .expect("authenticate_unknown_roles requires AuthenticateUnknown action");
    authenticated_roles_with(prev, session, SessionRoleAtom::Unknown)
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
        let state = spec
            .transition(
                &state,
                &SessionAuthAction::UploadClientAuthority(RemoteAuthorityAtom::Edge0),
            )
            .expect("authority upload should succeed");

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

    #[test]
    fn transition_program_matches_transition_function() {
        let spec = SessionAuthSpec::new();
        let program = spec.transition_program().expect("transition program");
        let initial = spec.initial_state();

        assert_eq!(
            program
                .evaluate(
                    &initial,
                    &SessionAuthAction::AcceptSession(SessionAtom::Session0),
                )
                .expect("evaluates"),
            transition_session_auth(
                &initial,
                &SessionAuthAction::AcceptSession(SessionAtom::Session0),
            )
        );
        assert_eq!(
            program
                .evaluate(
                    &initial,
                    &SessionAuthAction::AuthorizeClient(
                        StreamAtom::Stream0,
                        RequestKindAtom::HelloNegotiate,
                    ),
                )
                .expect("evaluates"),
            transition_session_auth(
                &initial,
                &SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::HelloNegotiate,
                ),
            )
        );
    }
}
