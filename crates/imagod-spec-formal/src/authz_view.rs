use imagod_spec::{
    AuthorizationDecision, ExternalMessage, OperationPermission, SessionId, SystemEvent,
    SystemStateFragment, TransportPrincipal,
};
use nirvash::BoolExpr;
use nirvash_lower::{DocStateProjection, ModelInstance};
use nirvash_macros::{invariant, nirvash_expr, nirvash_step_expr};

use crate::system::{SystemAction, SystemState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthzViewState {
    pub last_message_decision: AuthorizationDecision,
    pub last_operation_decision: AuthorizationDecision,
}

pub fn project(state: &SystemStateFragment) -> AuthzViewState {
    AuthzViewState {
        last_message_decision: state.last_message_decision,
        last_operation_decision: state.last_operation_decision,
    }
}

fn summarize_doc_state(state: &SystemState) -> nirvash::DocGraphState {
    nirvash::summarize_doc_graph_state(&project(state))
}

pub(crate) fn model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_authz_view")
            .with_checker_config(nirvash::ModelCheckConfig::reachable_graph())
            .with_doc_checker_config(crate::bounds::doc_cap_focus())
            .with_check_deadlocks(false)
            .with_doc_surface("Authorization View")
            .with_doc_state_projection(DocStateProjection::new(
                "AuthzViewState",
                summarize_doc_state,
            ))
            .with_action_constraint(nirvash_step_expr! { explicit_authz_view_actions(_prev, action, _next) =>
                matches!(action,
                    SystemEvent::LoadConfig(_)
                        | SystemEvent::FinishRestore
                        | SystemEvent::AcceptSession(_, _)
                        | SystemEvent::AuthenticateSession(_, _)
                        | SystemEvent::RequestMessage(_, _)
                        | SystemEvent::PrepareService(_)
                        | SystemEvent::CommitService(_)
                        | SystemEvent::PromoteService(_)
                        | SystemEvent::StartService(_)
                        | SystemEvent::VerifyManagerAuth(_)
                        | SystemEvent::GrantBinding(_)
                        | SystemEvent::RegisterRemoteAuthority(_)
                        | SystemEvent::ConnectRemoteRpc(_, _)
                        | SystemEvent::InvokeRemoteRpc(_, _, _, _)
                )
            }),
        ModelInstance::new("symbolic_authz_view")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Symbolic),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(crate::bounds::doc_cap_focus())
            .with_check_deadlocks(false)
            .with_doc_surface("Authorization View")
            .with_doc_state_projection(DocStateProjection::new(
                "AuthzViewState",
                summarize_doc_state,
            ))
            .with_action_constraint(nirvash_step_expr! { symbolic_authz_view_actions(_prev, action, _next) =>
                matches!(action,
                    SystemEvent::LoadConfig(_)
                        | SystemEvent::FinishRestore
                        | SystemEvent::AcceptSession(SessionId::Session0, _)
                        | SystemEvent::AuthenticateSession(SessionId::Session0, _)
                        | SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::CommandStart)
                        | SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::RpcInvoke)
                )
            }),
    ]
}

#[invariant(crate::system::SystemSpec)]
fn unknown_principal_never_gets_privileged_allow() -> BoolExpr<SystemState> {
    nirvash_expr! { unknown_principal_never_gets_privileged_allow(state) =>
        !(
            state.last_message == Some(ExternalMessage::CommandStart)
                && state.last_message_decision == AuthorizationDecision::Allow
                && (
                    state.last_message_session == Some(SessionId::Session0)
                        && state.session0_principal == TransportPrincipal::Unknown
                    || state.last_message_session == Some(SessionId::Session1)
                        && state.session1_principal == TransportPrincipal::Unknown
                )
        )
    }
}

#[invariant(crate::system::SystemSpec)]
fn remote_invoke_allow_implies_non_client_runner_role() -> BoolExpr<SystemState> {
    nirvash_expr! { remote_invoke_allow_implies_non_client_runner_role(state) =>
        state.last_operation_permission != Some(OperationPermission::RemoteInvoke)
            || state.last_operation_decision != AuthorizationDecision::Allow
            || (
                state.last_operation_actor == Some(imagod_spec::ServiceId::Service0)
                    || state.last_operation_actor == Some(imagod_spec::ServiceId::Service1)
            )
    }
}
