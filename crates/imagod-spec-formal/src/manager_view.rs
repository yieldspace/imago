use imagod_spec::{
    MaintenancePhase, ManagerPhase, ManagerShutdownPhase, SessionId, SystemEvent,
    SystemStateFragment,
};
use nirvash::BoolExpr;
use nirvash_lower::ModelInstance;
use nirvash_macros::{invariant, nirvash_expr, nirvash_step_expr};

use crate::system::{SystemAction, SystemSpec, SystemState};

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

pub(crate) fn model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_manager_view")
            .with_checker_config(nirvash::ModelCheckConfig::reachable_graph())
            .with_check_deadlocks(false)
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

#[invariant(SystemSpec)]
fn stopped_manager_rejects_control() -> BoolExpr<SystemState> {
    nirvash_expr! { stopped_manager_rejects_control(state) =>
        state.manager_phase != ManagerPhase::Stopped || !state.manager_accepts_control
    }
}
