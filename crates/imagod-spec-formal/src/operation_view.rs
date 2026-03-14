use imagod_spec::{
    CommandLifecycleState, OperationPermission, RpcOutcome, SystemEvent, SystemStateFragment,
};
use nirvash::BoolExpr;
use nirvash_lower::{DocStateProjection, ModelInstance};
use nirvash_macros::{invariant, nirvash_expr, nirvash_step_expr};

use crate::system::{SystemAction, SystemState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationViewState {
    pub command_state: Option<CommandLifecycleState>,
    pub last_permission: Option<OperationPermission>,
    pub rpc_outcome: RpcOutcome,
}

pub fn project(state: &SystemStateFragment) -> OperationViewState {
    OperationViewState {
        command_state: state.command_state,
        last_permission: state.last_operation_permission,
        rpc_outcome: state.last_rpc_outcome,
    }
}

fn summarize_doc_state(state: &SystemState) -> nirvash::DocGraphState {
    nirvash::summarize_doc_graph_state(&project(state))
}

pub(crate) fn model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_operation_view")
            .with_checker_config(nirvash::ModelCheckConfig::reachable_graph())
            .with_doc_checker_config(crate::bounds::doc_cap_focus())
            .with_check_deadlocks(false)
            .with_doc_surface("Operation View")
            .with_doc_state_projection(DocStateProjection::new(
                "OperationViewState",
                summarize_doc_state,
            ))
            .with_action_constraint(
                nirvash_step_expr! { explicit_operation_view_actions(_prev, action, _next) =>
                    matches!(action,
                        SystemEvent::LoadConfig(_)
                            | SystemEvent::FinishRestore
                            | SystemEvent::PrepareService(_)
                            | SystemEvent::CommitService(_)
                            | SystemEvent::PromoteService(_)
                            | SystemEvent::StartService(_)
                            | SystemEvent::VerifyManagerAuth(_)
                            | SystemEvent::GrantBinding(_)
                            | SystemEvent::RegisterRemoteAuthority(_)
                            | SystemEvent::StartCommand(_)
                            | SystemEvent::RequestCommandCancel
                            | SystemEvent::FinishCommand(_)
                            | SystemEvent::InvokeLocalRpc(_, _, _)
                            | SystemEvent::ConnectRemoteRpc(_, _)
                            | SystemEvent::InvokeRemoteRpc(_, _, _, _)
                            | SystemEvent::DisconnectRemoteRpc(_)
                    )
                },
            ),
    ]
}

#[invariant(crate::system::SystemSpec)]
fn stopped_manager_has_no_running_command() -> BoolExpr<SystemState> {
    nirvash_expr! { stopped_manager_has_no_running_command(state) =>
        state.manager_phase != imagod_spec::ManagerPhase::Stopped
            || state.command_state != Some(CommandLifecycleState::Running)
    }
}
