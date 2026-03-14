use imagod_spec::{BindingGrantId, ServiceLifecyclePhase, SystemEvent, SystemStateFragment};
use nirvash::BoolExpr;
use nirvash_lower::{DocStateProjection, ModelInstance};
use nirvash_macros::{invariant, nirvash_expr, nirvash_step_expr};

use crate::system::{SystemAction, SystemState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceViewState {
    pub service0: ServiceLifecyclePhase,
    pub service1: ServiceLifecyclePhase,
    pub binding_count: usize,
}

pub fn project(state: &SystemStateFragment) -> ServiceViewState {
    let binding_count = [
        BindingGrantId::Service0ToService1ControlApi,
        BindingGrantId::Service0ToService1LogsApi,
        BindingGrantId::Service1ToService0ControlApi,
        BindingGrantId::Service1ToService0LogsApi,
    ]
    .into_iter()
    .filter(|grant| state.binding_grants.contains(grant))
    .count();

    ServiceViewState {
        service0: state.service0_lifecycle,
        service1: state.service1_lifecycle,
        binding_count,
    }
}

fn summarize_doc_state(state: &SystemState) -> nirvash::DocGraphState {
    nirvash::summarize_doc_graph_state(&project(state))
}

pub(crate) fn model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_service_view")
            .with_checker_config(nirvash::ModelCheckConfig::reachable_graph())
            .with_doc_checker_config(crate::bounds::doc_cap_focus())
            .with_check_deadlocks(false)
            .with_doc_surface("Service View")
            .with_doc_state_projection(DocStateProjection::new(
                "ServiceViewState",
                summarize_doc_state,
            ))
            .with_action_constraint(
                nirvash_step_expr! { explicit_service_view_actions(_prev, action, _next) =>
                    matches!(action,
                        SystemEvent::LoadConfig(_)
                            | SystemEvent::FinishRestore
                            | SystemEvent::PrepareService(_)
                            | SystemEvent::CommitService(_)
                            | SystemEvent::PromoteService(_)
                            | SystemEvent::StartService(_)
                            | SystemEvent::StopService(_)
                            | SystemEvent::GrantBinding(_)
                            | SystemEvent::RequestShutdown
                            | SystemEvent::ConfirmServicesStopped
                    )
                },
            ),
    ]
}

#[invariant(crate::system::SystemSpec)]
fn reaped_services_carry_no_live_rpc_targets() -> BoolExpr<SystemState> {
    nirvash_expr! { reaped_services_carry_no_live_rpc_targets(state) =>
        !(state.service0_lifecycle == ServiceLifecyclePhase::Reaped && state.local_rpc_target == Some(imagod_spec::ServiceId::Service0))
            && !(state.service1_lifecycle == ServiceLifecyclePhase::Reaped && state.local_rpc_target == Some(imagod_spec::ServiceId::Service1))
    }
}
