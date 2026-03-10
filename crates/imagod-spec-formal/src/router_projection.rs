use imagod_spec::{RouterOutputSummary, RouterStateSummary, SummaryRequestKind};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature};

use crate::{
    CommandKind, CommandProtocolAction,
    atoms::{RequestKindAtom, SessionAtom, StreamAtom},
    session_auth::SessionAuthState,
    session_auth::SessionAuthAction,
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

    fn request_kind(self, action: RouterProjectionAction) -> SummaryRequestKind {
        match action {
            RouterProjectionAction::HelloNegotiate => SummaryRequestKind::HelloNegotiate,
            RouterProjectionAction::DeployPrepare => SummaryRequestKind::DeployPrepare,
            RouterProjectionAction::ArtifactPush => SummaryRequestKind::ArtifactPush,
            RouterProjectionAction::ArtifactCommit => SummaryRequestKind::ArtifactCommit,
            RouterProjectionAction::StateRequest => SummaryRequestKind::StateRequest,
            RouterProjectionAction::ServicesList => SummaryRequestKind::ServicesList,
            RouterProjectionAction::CommandCancel => SummaryRequestKind::CommandCancel,
            RouterProjectionAction::RpcInvoke => SummaryRequestKind::RpcInvoke,
            RouterProjectionAction::BindingsCertUpload => SummaryRequestKind::BindingsCertUpload,
        }
    }

    pub fn initial_summary(self) -> RouterStateSummary {
        RouterStateSummary::initial_admin_stream()
    }

    pub fn action_allowed(self, summary: &RouterStateSummary, action: RouterProjectionAction) -> bool {
        if !summary.active_session || summary.request.is_some() {
            return false;
        }
        match action {
            RouterProjectionAction::HelloNegotiate => true,
            RouterProjectionAction::DeployPrepare => summary.deploy_prepare_authorized,
            RouterProjectionAction::ArtifactPush => summary.artifact_push_authorized,
            RouterProjectionAction::ArtifactCommit => summary.artifact_commit_authorized,
            RouterProjectionAction::StateRequest => summary.state_request_authorized,
            RouterProjectionAction::ServicesList => summary.services_list_authorized,
            RouterProjectionAction::CommandCancel => summary.command_cancel_authorized,
            RouterProjectionAction::RpcInvoke => summary.rpc_invoke_authorized,
            RouterProjectionAction::BindingsCertUpload => summary.bindings_cert_upload_authorized,
        }
    }

    pub fn advance_summary(
        self,
        summary: &RouterStateSummary,
        action: RouterProjectionAction,
    ) -> RouterStateSummary {
        let mut next = *summary;
        next.request = Some(self.request_kind(action));
        if matches!(action, RouterProjectionAction::BindingsCertUpload) {
            next.authority_uploaded = true;
        }
        next
    }

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

impl ProtocolConformanceSpec for RouterProjectionSpec {
    type ExpectedOutput = Vec<SystemEffect>;
    type SummaryState = RouterStateSummary;
    type SummaryOutput = RouterOutputSummary;

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
        state.session = SessionTransportState::from_summary(summary.active_session, false);
        state.session_auth = SessionAuthState::from_router_summary(summary);
        state.wire = crate::wire_protocol::WireProtocolState::from_router_summary(summary);
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
        let state = spec
            .transition(
                &spec.initial_state(),
                &RouterProjectionAction::BindingsCertUpload,
            )
            .expect("bindings.cert.upload should be allowed");

        assert!(
            state
                .wire
                .saw_request(StreamAtom::Stream0, RequestKindAtom::BindingsCertUpload)
        );
        assert!(
            state
                .session_auth
                .authority_uploaded(crate::atoms::RemoteAuthorityAtom::Edge0)
        );
    }
}
