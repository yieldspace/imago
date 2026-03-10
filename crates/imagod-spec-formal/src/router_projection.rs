#[cfg(test)]
use imagod_spec::{ContractEffectSummary, SummaryRequestKind, SummaryStreamId};
use imagod_spec::{RouterOutputSummary, RouterProbeOutput, RouterProbeState, RouterStateSummary};
use nirvash_core::{
    ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{ActionVocabulary, Signature, nirvash_projection_model};

use crate::{
    CommandKind, CommandProtocolAction,
    atoms::{RemoteAuthorityAtom, RequestKindAtom, SessionAtom, SessionRoleAtom, StreamAtom},
    session_auth::SessionAuthAction,
    session_auth::SessionAuthState,
    session_transport::SessionTransportState,
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

    fn state_from_summary(self, summary: &RouterStateSummary) -> SystemState {
        let mut state = self.initial_state();
        state.session = SessionTransportState::from_summary(summary.active_session, false);
        state.session_auth = SessionAuthState::from_router_summary(summary);
        state.wire = crate::wire_protocol::WireProtocolState::from_router_summary(summary);
        state
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
        self.system()
            .transition(
                state,
                &nirvash_core::concurrent::ConcurrentAction::from_atomic(SystemAtomicAction::Wire(
                    self.wire_action(*action),
                )),
            )
            .map(|next| {
                let role = if next
                    .session_auth
                    .any_authenticated_as(SessionRoleAtom::Admin)
                {
                    Some(imagod_spec::SummarySessionRole::Admin)
                } else if next
                    .session_auth
                    .any_authenticated_as(SessionRoleAtom::Client)
                {
                    Some(imagod_spec::SummarySessionRole::Client)
                } else if next
                    .session_auth
                    .any_authenticated_as(SessionRoleAtom::Unknown)
                {
                    Some(imagod_spec::SummarySessionRole::Unknown)
                } else {
                    None
                };
                let probe = RouterProbeState {
                    active_session: next.session.has_active_sessions(),
                    role,
                    deploy_prepare_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::DeployPrepare),
                    artifact_push_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::ArtifactPush),
                    artifact_commit_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::ArtifactCommit),
                    state_request_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::StateRequest),
                    services_list_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::ServicesList),
                    command_cancel_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::CommandCancel),
                    rpc_invoke_authorized: next
                        .session_auth
                        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::RpcInvoke),
                    bindings_cert_upload_authorized: next.session_auth.stream_authorized(
                        StreamAtom::Stream0,
                        RequestKindAtom::BindingsCertUpload,
                    ),
                    authority_uploaded: next
                        .session_auth
                        .authority_uploaded(RemoteAuthorityAtom::Edge0),
                };
                let summary = <Self as ProtocolConformanceSpec>::summarize_state(self, &probe);
                <Self as ProtocolConformanceSpec>::abstract_state(self, &summary)
            })
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

#[cfg(test)]
fn router_probe_state_domain() -> nirvash_core::BoundedDomain<RouterProbeState> {
    <RouterProbeState as nirvash_core::Signature>::bounded_domain()
}

#[cfg(test)]
fn router_summary_output_domain() -> nirvash_core::BoundedDomain<RouterOutputSummary> {
    let mut values = vec![RouterOutputSummary::default()];
    for kind in [
        SummaryRequestKind::HelloNegotiate,
        SummaryRequestKind::DeployPrepare,
        SummaryRequestKind::ArtifactPush,
        SummaryRequestKind::ArtifactCommit,
        SummaryRequestKind::StateRequest,
        SummaryRequestKind::ServicesList,
        SummaryRequestKind::CommandCancel,
        SummaryRequestKind::RpcInvoke,
        SummaryRequestKind::BindingsCertUpload,
    ] {
        values.push(RouterOutputSummary {
            effects: vec![ContractEffectSummary::RequestObserved(
                SummaryStreamId::Stream0,
                kind,
            )],
        });
        values.push(RouterOutputSummary {
            effects: vec![
                ContractEffectSummary::RequestObserved(SummaryStreamId::Stream0, kind),
                ContractEffectSummary::Response(SummaryStreamId::Stream0, kind),
            ],
        });
    }
    nirvash_core::BoundedDomain::new(values)
}

nirvash_projection_model! {
    probe_state = RouterProbeState,
    probe_output = RouterProbeOutput,
    summary_state = RouterStateSummary,
    summary_output = RouterOutputSummary,
    abstract_state = SystemState,
    expected_output = Vec<SystemEffect>,
    probe_state_domain = router_probe_state_domain,
    summary_output_domain = router_summary_output_domain,
    state_seed = spec.initial_state(),
    state_summary {
        active_session <= probe.active_session,
        role <= probe.role,
        deploy_prepare_authorized <= probe.deploy_prepare_authorized,
        artifact_push_authorized <= probe.artifact_push_authorized,
        artifact_commit_authorized <= probe.artifact_commit_authorized,
        state_request_authorized <= probe.state_request_authorized,
        services_list_authorized <= probe.services_list_authorized,
        command_cancel_authorized <= probe.command_cancel_authorized,
        rpc_invoke_authorized <= probe.rpc_invoke_authorized,
        bindings_cert_upload_authorized <= probe.bindings_cert_upload_authorized,
        authority_uploaded <= probe.authority_uploaded,
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
            .expect("router projection response should map to SystemEffect"),
        imagod_spec::ContractEffectSummary::AuthorizationGranted(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::CommandEvent(_, _) => crate::summary_mapping::system_effect(effect)
            .expect("router projection command event should map to SystemEffect"),
        effect @ imagod_spec::ContractEffectSummary::LogChunk(_, _) => crate::summary_mapping::system_effect(effect)
            .expect("router projection log chunk should map to SystemEffect"),
        effect @ imagod_spec::ContractEffectSummary::LogsEnd(_) => crate::summary_mapping::system_effect(effect)
            .expect("router projection logs end should map to SystemEffect"),
        effect @ imagod_spec::ContractEffectSummary::AuthorizationRejected(_, _) => crate::summary_mapping::system_effect(effect)
            .expect("router projection authorization rejection should map to SystemEffect"),
        imagod_spec::ContractEffectSummary::LocalRpcResolved(_) => drop,
        imagod_spec::ContractEffectSummary::LocalRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcConnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcCompleted(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDisconnected(_) => drop,
        imagod_spec::ContractEffectSummary::RemoteRpcDenied(_) => drop,
        imagod_spec::ContractEffectSummary::TaskMilestone(_, _) => drop,
        effect @ imagod_spec::ContractEffectSummary::ShutdownComplete => crate::summary_mapping::system_effect(effect)
            .expect("router projection shutdown completion should map to SystemEffect"),
    }
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
            .transition(&prev, &RouterProjectionAction::BindingsCertUpload)
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
