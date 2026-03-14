use imagod_spec::{
    MaintenancePhase, ManagerPhase, ManagerShutdownPhase, SessionId, SystemEvent,
    SystemStateFragment,
};
use nirvash::BoolExpr;
use nirvash_lower::{DocStateProjection, ModelInstance};
use nirvash_macros::{invariant, nirvash_expr, nirvash_step_expr};

use crate::system::{SystemAction, SystemState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerViewState {
    pub phase: ManagerPhase,
    pub shutdown_phase: ManagerShutdownPhase,
    pub accepts_control: bool,
    pub maintenance: MaintenancePhase,
}

pub fn project(state: &SystemStateFragment) -> ManagerViewState {
    ManagerViewState {
        phase: state.manager_phase,
        shutdown_phase: state.manager_shutdown_phase,
        accepts_control: state.manager_accepts_control,
        maintenance: state.maintenance_phase,
    }
}

fn summarize_doc_state(state: &SystemState) -> nirvash::DocGraphState {
    nirvash::summarize_doc_graph_state(&project(state))
}

pub(crate) fn model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_manager_view")
            .with_checker_config(nirvash::ModelCheckConfig::reachable_graph())
            .with_doc_checker_config(crate::bounds::doc_cap_focus())
            .with_check_deadlocks(false)
            .with_doc_surface("Manager View")
            .with_doc_state_projection(DocStateProjection::new(
                "ManagerViewState",
                summarize_doc_state,
            ))
            .with_action_constraint(
                nirvash_step_expr! { explicit_manager_view_actions(_prev, action, _next) =>
                    matches!(action,
                        SystemEvent::LoadConfig(_)
                            | SystemEvent::FinishRestore
                            | SystemEvent::RequestShutdown
                            | SystemEvent::DrainSession(SessionId::Session0)
                            | SystemEvent::ConfirmSessionsDrained
                            | SystemEvent::ConfirmServicesStopped
                            | SystemEvent::ConfirmMaintenanceStopped
                            | SystemEvent::CompleteShutdown
                    )
                },
            ),
    ]
}

#[invariant(crate::system::SystemSpec)]
fn stopped_manager_rejects_control() -> BoolExpr<SystemState> {
    nirvash_expr! { stopped_manager_rejects_control(state) =>
        state.manager_phase != ManagerPhase::Stopped || !state.manager_accepts_control
    }
}
