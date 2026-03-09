use imago_protocol::CommandProtocolAction;
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature};

use crate::{
    atoms::{RequestKindAtom, SessionAtom, StreamAtom},
    session_auth::SessionAuthAction,
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RouterProjectionObservedState {
    pub trace: Vec<RouterProjectionAction>,
}

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
            SystemAtomicAction::Command(CommandProtocolAction::Start(
                imago_protocol::CommandKind::Deploy,
            )),
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
    type ObservedState = RouterProjectionObservedState;
    type ObservedOutput = Vec<SystemEffect>;

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

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State {
        observed.trace.iter().fold(self.initial_state(), |state, action| {
            self.transition(&state, action)
                .expect("router projection trace should stay valid")
        })
    }

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput {
        observed.clone()
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
