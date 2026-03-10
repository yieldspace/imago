use imagod_spec::{SessionAuthOutputSummary, SessionAuthStateSummary};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature};

use crate::{
    atoms::{RemoteAuthorityAtom, RequestKindAtom, SessionAtom, StreamAtom},
    session_auth::SessionAuthAction,
    session_auth::SessionAuthState,
    session_transport::SessionTransportState,
    summary_mapping::system_effects,
    system::{SystemAtomicAction, SystemEffect, SystemSpec, SystemState},
};

/// Session/auth surface projected from the unified `system` spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum SessionAuthProjectionAction {
    /// Accept one session.
    AcceptSession,
    /// Authenticate that session as admin.
    AuthenticateAdmin,
    /// Authenticate that session as client.
    AuthenticateClient,
    /// Authenticate that session as unknown.
    AuthenticateUnknown,
    /// Authorize `services.list` on one stream for admin.
    AuthorizeAdminServicesList,
    /// Authorize `hello.negotiate` on one stream for client.
    AuthorizeClientHello,
    /// Authorize `rpc.invoke` on one stream for client.
    AuthorizeClientRpc,
    /// Reject unauthorized `services.list`.
    RejectUnauthorizedServicesList,
    /// Record read timeout for one stream.
    ReadTimeout,
    /// Close one stream.
    CloseStream,
    /// Register one uploaded client authority.
    UploadClientAuthority,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SessionAuthProjectionSpec;

impl SessionAuthProjectionSpec {
    pub const fn new() -> Self {
        Self
    }

    fn system(self) -> SystemSpec {
        SystemSpec::new()
    }

    pub fn initial_state(self) -> SystemState {
        self.system().initial_state()
    }

    fn projected_action(self, action: SessionAuthProjectionAction) -> SessionAuthAction {
        match action {
            SessionAuthProjectionAction::AcceptSession => {
                SessionAuthAction::AcceptSession(SessionAtom::Session0)
            }
            SessionAuthProjectionAction::AuthenticateAdmin => {
                SessionAuthAction::AuthenticateAdmin(SessionAtom::Session0)
            }
            SessionAuthProjectionAction::AuthenticateClient => {
                SessionAuthAction::AuthenticateClient(SessionAtom::Session0)
            }
            SessionAuthProjectionAction::AuthenticateUnknown => {
                SessionAuthAction::AuthenticateUnknown(SessionAtom::Session0)
            }
            SessionAuthProjectionAction::AuthorizeAdminServicesList => {
                SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream0,
                    RequestKindAtom::ServicesList,
                )
            }
            SessionAuthProjectionAction::AuthorizeClientHello => {
                SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::HelloNegotiate,
                )
            }
            SessionAuthProjectionAction::AuthorizeClientRpc => {
                SessionAuthAction::AuthorizeClient(StreamAtom::Stream0, RequestKindAtom::RpcInvoke)
            }
            SessionAuthProjectionAction::RejectUnauthorizedServicesList => {
                SessionAuthAction::RejectUnauthorized(
                    StreamAtom::Stream0,
                    RequestKindAtom::ServicesList,
                )
            }
            SessionAuthProjectionAction::ReadTimeout => {
                SessionAuthAction::ReadTimeout(StreamAtom::Stream0)
            }
            SessionAuthProjectionAction::CloseStream => {
                SessionAuthAction::CloseStream(StreamAtom::Stream0)
            }
            SessionAuthProjectionAction::UploadClientAuthority => {
                SessionAuthAction::UploadClientAuthority(RemoteAuthorityAtom::Edge0)
            }
        }
    }

    pub fn initial_summary(self) -> SessionAuthStateSummary {
        SessionAuthStateSummary::default()
    }

    pub fn action_allowed(
        self,
        summary: &SessionAuthStateSummary,
        action: SessionAuthProjectionAction,
    ) -> bool {
        match action {
            SessionAuthProjectionAction::AcceptSession => {
                !summary.active_session && !summary.shutdown_requested
            }
            SessionAuthProjectionAction::AuthenticateAdmin
            | SessionAuthProjectionAction::AuthenticateClient
            | SessionAuthProjectionAction::AuthenticateUnknown => {
                summary.active_session && summary.role.is_none()
            }
            SessionAuthProjectionAction::AuthorizeAdminServicesList => {
                summary.active_session
                    && matches!(summary.role, Some(imagod_spec::SummarySessionRole::Admin))
                    && !summary.shutdown_requested
                    && !summary.read_timed_out
                    && !summary.stream_closed
                    && !summary.admin_services_list_authorized
            }
            SessionAuthProjectionAction::AuthorizeClientHello => {
                summary.active_session
                    && matches!(summary.role, Some(imagod_spec::SummarySessionRole::Client))
                    && !summary.shutdown_requested
                    && !summary.read_timed_out
                    && !summary.stream_closed
                    && !summary.client_hello_authorized
            }
            SessionAuthProjectionAction::AuthorizeClientRpc => {
                summary.active_session
                    && matches!(summary.role, Some(imagod_spec::SummarySessionRole::Client))
                    && !summary.shutdown_requested
                    && !summary.read_timed_out
                    && !summary.stream_closed
                    && summary.client_authority_uploaded
                    && !summary.client_rpc_authorized
            }
            SessionAuthProjectionAction::RejectUnauthorizedServicesList => {
                !summary.admin_services_list_authorized
            }
            SessionAuthProjectionAction::ReadTimeout => !summary.stream_closed,
            SessionAuthProjectionAction::CloseStream => !summary.stream_closed,
            SessionAuthProjectionAction::UploadClientAuthority => {
                !summary.shutdown_requested && !summary.client_authority_uploaded
            }
        }
    }

    pub fn advance_summary(
        self,
        summary: &SessionAuthStateSummary,
        action: SessionAuthProjectionAction,
    ) -> SessionAuthStateSummary {
        let mut next = *summary;
        match action {
            SessionAuthProjectionAction::AcceptSession => next.active_session = true,
            SessionAuthProjectionAction::AuthenticateAdmin => {
                next.role = Some(imagod_spec::SummarySessionRole::Admin);
            }
            SessionAuthProjectionAction::AuthenticateClient => {
                next.role = Some(imagod_spec::SummarySessionRole::Client);
            }
            SessionAuthProjectionAction::AuthenticateUnknown => {
                next.role = Some(imagod_spec::SummarySessionRole::Unknown);
            }
            SessionAuthProjectionAction::AuthorizeAdminServicesList => {
                next.admin_services_list_authorized = true;
            }
            SessionAuthProjectionAction::AuthorizeClientHello => {
                next.client_hello_authorized = true;
            }
            SessionAuthProjectionAction::AuthorizeClientRpc => {
                next.client_rpc_authorized = true;
            }
            SessionAuthProjectionAction::RejectUnauthorizedServicesList => {
                next.unauthorized_services_list_rejected = true;
            }
            SessionAuthProjectionAction::ReadTimeout => {
                next.read_timed_out = true;
                next.admin_services_list_authorized = false;
                next.client_hello_authorized = false;
                next.client_rpc_authorized = false;
            }
            SessionAuthProjectionAction::CloseStream => {
                next.stream_closed = true;
                next.admin_services_list_authorized = false;
                next.client_hello_authorized = false;
                next.client_rpc_authorized = false;
            }
            SessionAuthProjectionAction::UploadClientAuthority => {
                next.client_authority_uploaded = true;
            }
        }
        next
    }
}

impl TransitionSystem for SessionAuthProjectionSpec {
    type State = SystemState;
    type Action = SessionAuthProjectionAction;

    fn name(&self) -> &'static str {
        "session_auth_projection"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.system().transition(
            state,
            &nirvash_core::concurrent::ConcurrentAction::from_atomic(
                SystemAtomicAction::SessionAuth(self.projected_action(*action)),
            ),
        )
    }
}

impl TemporalSpec for SessionAuthProjectionSpec {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
        self.system().invariants()
    }
}

impl ModelCaseSource for SessionAuthProjectionSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_check_deadlocks(false)]
    }
}

impl ProtocolConformanceSpec for SessionAuthProjectionSpec {
    type ExpectedOutput = Vec<SystemEffect>;
    type SummaryState = SessionAuthStateSummary;
    type SummaryOutput = SessionAuthOutputSummary;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        self.system().expected_output(
            prev,
            &nirvash_core::concurrent::ConcurrentAction::from_atomic(
                SystemAtomicAction::SessionAuth(self.projected_action(*action)),
            ),
            next,
        )
    }

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
        let mut state = self.initial_state();
        state.session =
            SessionTransportState::from_summary(summary.active_session, summary.shutdown_requested);
        state.session_auth = SessionAuthState::from_summary(summary);
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
    fn client_rpc_requires_uploaded_authority_in_projection() {
        let spec = SessionAuthProjectionSpec::new();
        let accepted = spec
            .transition(
                &spec.initial_state(),
                &SessionAuthProjectionAction::AcceptSession,
            )
            .expect("accept should be allowed");
        let authenticated = spec
            .transition(&accepted, &SessionAuthProjectionAction::AuthenticateClient)
            .expect("authenticate client should be allowed");

        assert!(
            spec.transition(
                &authenticated,
                &SessionAuthProjectionAction::AuthorizeClientRpc,
            )
            .is_none()
        );

        let uploaded = spec
            .transition(
                &authenticated,
                &SessionAuthProjectionAction::UploadClientAuthority,
            )
            .expect("authority upload should be allowed");
        let authorized = spec
            .transition(&uploaded, &SessionAuthProjectionAction::AuthorizeClientRpc)
            .expect("rpc authorization should be allowed after upload");

        assert!(
            authorized
                .session_auth
                .stream_authorized(StreamAtom::Stream0, RequestKindAtom::RpcInvoke)
        );
    }

    #[test]
    fn reject_unauthorized_emits_system_effect() {
        let spec = SessionAuthProjectionSpec::new();
        let accepted = spec
            .transition(
                &spec.initial_state(),
                &SessionAuthProjectionAction::AcceptSession,
            )
            .expect("accept should be allowed");
        let authenticated = spec
            .transition(&accepted, &SessionAuthProjectionAction::AuthenticateUnknown)
            .expect("authenticate unknown should be allowed");
        let next = spec
            .transition(
                &authenticated,
                &SessionAuthProjectionAction::RejectUnauthorizedServicesList,
            )
            .expect("reject unauthorized should be allowed");

        assert_eq!(
            spec.expected_output(
                &authenticated,
                &SessionAuthProjectionAction::RejectUnauthorizedServicesList,
                Some(&next),
            ),
            vec![SystemEffect::AuthorizationRejected(
                StreamAtom::Stream0,
                RequestKindAtom::ServicesList,
            )]
        );
    }
}
