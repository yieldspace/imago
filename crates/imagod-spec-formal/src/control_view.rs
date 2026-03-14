use imagod_spec::{
    SessionAuthState, SessionId, SessionRequestState, SystemEvent, SystemStateFragment,
};
use nirvash::BoolExpr;
use nirvash_lower::{DocStateProjection, ModelInstance};
use nirvash_macros::{invariant, nirvash_expr, nirvash_step_expr};

use crate::system::{SystemAction, SystemState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlViewState {
    pub active_sessions: Vec<SessionId>,
    pub session0_auth: SessionAuthState,
    pub session1_auth: SessionAuthState,
    pub session0_request: SessionRequestState,
    pub session1_request: SessionRequestState,
}

pub fn project(state: &SystemStateFragment) -> ControlViewState {
    let mut active_sessions = Vec::new();
    if state.active_sessions.contains(&SessionId::Session0) {
        active_sessions.push(SessionId::Session0);
    }
    if state.active_sessions.contains(&SessionId::Session1) {
        active_sessions.push(SessionId::Session1);
    }
    ControlViewState {
        active_sessions,
        session0_auth: state.session0_auth,
        session1_auth: state.session1_auth,
        session0_request: state.session0_request,
        session1_request: state.session1_request,
    }
}

fn summarize_doc_state(state: &SystemState) -> nirvash::DocGraphState {
    nirvash::summarize_doc_graph_state(&project(state))
}

pub(crate) fn model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_control_view")
            .with_checker_config(nirvash::ModelCheckConfig::reachable_graph())
            .with_doc_checker_config(crate::bounds::doc_cap_focus())
            .with_check_deadlocks(false)
            .with_doc_surface("Control View")
            .with_doc_state_projection(DocStateProjection::new(
                "ControlViewState",
                summarize_doc_state,
            ))
            .with_action_constraint(
                nirvash_step_expr! { explicit_control_view_actions(_prev, action, _next) =>
                    matches!(action,
                        SystemEvent::LoadConfig(_)
                            | SystemEvent::FinishRestore
                            | SystemEvent::AcceptSession(_, _)
                            | SystemEvent::AuthenticateSession(_, _)
                            | SystemEvent::RequestMessage(_, _)
                            | SystemEvent::CompleteMessage(_)
                            | SystemEvent::DrainSession(_)
                            | SystemEvent::RequestShutdown
                            | SystemEvent::ConfirmSessionsDrained
                    )
                },
            ),
    ]
}

#[invariant(crate::system::SystemSpec)]
fn drained_sessions_are_quiescent() -> BoolExpr<SystemState> {
    nirvash_expr! { drained_sessions_are_quiescent(state) =>
        (state.session0_auth != SessionAuthState::Drained || state.session0_request == SessionRequestState::Idle)
            && (state.session1_auth != SessionAuthState::Drained || state.session1_request == SessionRequestState::Idle)
    }
}
