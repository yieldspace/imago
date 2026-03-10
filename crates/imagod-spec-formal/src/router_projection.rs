use imagod_spec::{RouterOutputSummary, RouterProbeOutput, RouterProbeState, RouterStateSummary};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature, nirvash_projection_contract};

use crate::{
    CommandKind, CommandProtocolAction,
    atoms::{RemoteAuthorityAtom, RequestKindAtom, SessionAtom, SessionRoleAtom, StreamAtom},
    session_auth::SessionAuthAction,
    session_auth::SessionAuthState,
    session_transport::SessionTransportState,
    summary_mapping::system_effects,
    system::{SystemAtomicAction, SystemEffect, SystemSpec, SystemState},
    wire_protocol::WireProtocolAction,
};

/// Request/response router surface projected from the unified `system` spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum RouterProjectionAction {
    /// Observe `hello.negotiate`.
    HelloNegotiate,
    /// Observe `deploy.prepare`.
    DeployPrepare,
    /// Observe `artifact.push`.
    ArtifactPush,
    /// Observe `artifact.commit`.
    ArtifactCommit,
    /// Observe `state.request`.
    StateRequest,
    /// Observe `services.list`.
    ServicesList,
    /// Observe `command.cancel`.
    CommandCancel,
    /// Observe `rpc.invoke`.
    RpcInvoke,
    /// Observe `bindings.cert.upload`.
    BindingsCertUpload,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RouterProjectionSpec;

impl RouterProjectionSpec {
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
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::Start(CommandKind::Deploy)),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::DeployPrepare,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::ArtifactPush,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::ArtifactCommit,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::StateRequest,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::ServicesList,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::CommandCancel,
            )),
        );
        let state = self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::RpcInvoke,
            )),
        );
        self.apply_atomic(
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::BindingsCertUpload,
            )),
        )
    }

    fn wire_action(self, action: RouterProjectionAction) -> WireProtocolAction {
        match action {
            RouterProjectionAction::HelloNegotiate => {
                WireProtocolAction::HelloNegotiate(StreamAtom::Stream0)
            }
            RouterProjectionAction::DeployPrepare => {
                WireProtocolAction::DeployPrepare(StreamAtom::Stream0)
            }
            RouterProjectionAction::ArtifactPush => {
                WireProtocolAction::ArtifactPush(StreamAtom::Stream0)
            }
            RouterProjectionAction::ArtifactCommit => {
                WireProtocolAction::ArtifactCommit(StreamAtom::Stream0)
            }
            RouterProjectionAction::StateRequest => {
                WireProtocolAction::StateRequest(StreamAtom::Stream0)
            }
            RouterProjectionAction::ServicesList => {
                WireProtocolAction::ServicesList(StreamAtom::Stream0)
            }
            RouterProjectionAction::CommandCancel => {
                WireProtocolAction::CommandCancel(StreamAtom::Stream0)
            }
            RouterProjectionAction::RpcInvoke => WireProtocolAction::RpcInvoke(StreamAtom::Stream0),
            RouterProjectionAction::BindingsCertUpload => {
                WireProtocolAction::BindingsCertUpload(StreamAtom::Stream0)
            }
        }
    }
}

fn summarize_router_state(probe: &RouterProbeState) -> RouterStateSummary {
    (*probe).into()
}

fn summarize_router_output(probe: &RouterProbeOutput) -> RouterOutputSummary {
    probe.output.clone()
}

fn abstract_router_state(spec: &RouterProjectionSpec, summary: &RouterStateSummary) -> SystemState {
    let mut state = spec.initial_state();
    state.session = SessionTransportState::from_summary(summary.active_session, false);
    state.session_auth = SessionAuthState::from_router_summary(summary);
    state.wire = crate::wire_protocol::WireProtocolState::from_router_summary(summary);
    state
}

fn abstract_router_output(
    _spec: &RouterProjectionSpec,
    summary: &RouterOutputSummary,
) -> Vec<SystemEffect> {
    system_effects(&summary.effects)
}

fn router_summary_from_state(state: &SystemState) -> RouterStateSummary {
    let role = if state.session_auth.any_authenticated_as(SessionRoleAtom::Admin) {
        Some(imagod_spec::SummarySessionRole::Admin)
    } else if state
        .session_auth
        .any_authenticated_as(SessionRoleAtom::Client)
    {
        Some(imagod_spec::SummarySessionRole::Client)
    } else if state
        .session_auth
        .any_authenticated_as(SessionRoleAtom::Unknown)
    {
        Some(imagod_spec::SummarySessionRole::Unknown)
    } else {
        None
    };

    RouterStateSummary {
        active_session: state.session.has_active_sessions(),
        role,
        deploy_prepare_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::DeployPrepare),
        artifact_push_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::ArtifactPush),
        artifact_commit_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::ArtifactCommit),
        state_request_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::StateRequest),
        services_list_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::ServicesList),
        command_cancel_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::CommandCancel),
        rpc_invoke_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::RpcInvoke),
        bindings_cert_upload_authorized: state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, RequestKindAtom::BindingsCertUpload),
        authority_uploaded: state
            .session_auth
            .authority_uploaded(RemoteAuthorityAtom::Edge0),
    }
}

fn normalize_router_state(spec: RouterProjectionSpec, state: SystemState) -> SystemState {
    let summary = router_summary_from_state(&state);
    abstract_router_state(&spec, &summary)
}

impl TransitionSystem for RouterProjectionSpec {
    type State = SystemState;
    type Action = RouterProjectionAction;

    fn name(&self) -> &'static str {
        "router_projection"
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
            &nirvash_core::concurrent::ConcurrentAction::from_atomic(SystemAtomicAction::Wire(
                self.wire_action(*action),
            )),
        )
        .map(|next| normalize_router_state(*self, next))
    }
}

impl TemporalSpec for RouterProjectionSpec {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
        self.system().invariants()
    }
}

impl ModelCaseSource for RouterProjectionSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_check_deadlocks(false)]
    }
}

#[nirvash_projection_contract(
    probe_state = RouterProbeState,
    probe_output = RouterProbeOutput,
    summary_state = RouterStateSummary,
    summary_output = RouterOutputSummary,
    summarize_state = summarize_router_state,
    summarize_output = summarize_router_output,
    abstract_state = abstract_router_state,
    abstract_output = abstract_router_output
)]
impl ProtocolConformanceSpec for RouterProjectionSpec {
    type ExpectedOutput = Vec<SystemEffect>;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_starts_listening_with_admin_router_stream() {
        let state = RouterProjectionSpec::new().initial_state();

        assert!(state.session.has_active_sessions());
        assert!(
            state
                .session_auth
                .stream_authorized(StreamAtom::Stream0, RequestKindAtom::DeployPrepare)
        );
        assert!(
            state
                .session_auth
                .stream_authorized(StreamAtom::Stream0, RequestKindAtom::BindingsCertUpload)
        );
    }

    #[test]
    fn bindings_cert_upload_updates_authority_projection() {
        let spec = RouterProjectionSpec::new();
        let prev = spec.initial_state();
        let state = spec
            .transition(
                &prev,
                &RouterProjectionAction::BindingsCertUpload,
            )
            .expect("bindings.cert.upload should be allowed");

        assert_eq!(
            spec.expected_output(
                &prev,
                &RouterProjectionAction::BindingsCertUpload,
                Some(&state)
            ),
            vec![SystemEffect::Response(
                StreamAtom::Stream0,
                RequestKindAtom::BindingsCertUpload,
            )]
        );
        assert!(
            state
                .session_auth
                .authority_uploaded(crate::atoms::RemoteAuthorityAtom::Edge0)
        );
    }
}
