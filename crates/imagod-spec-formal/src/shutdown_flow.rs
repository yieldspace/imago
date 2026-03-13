use nirvash::{BoolExpr, Fairness, Ltl, ModelCase, TransitionSystem};
use nirvash_macros::{
    ActionVocabulary, Signature, fairness, invariant, nirvash_expr, nirvash_step_expr,
    nirvash_transition_program, property, subsystem_spec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum ShutdownPhase {
    Idle,
    SignalReceived,
    DrainingSessions,
    StoppingServices,
    StoppingMaintenance,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub struct ShutdownFlowState {
    pub phase: ShutdownPhase,
    pub accepts_stopped: bool,
    pub sessions_drained: bool,
    pub services_stopped: bool,
    pub maintenance_stopped: bool,
    pub forced_stop_attempted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum ShutdownFlowAction {
    /// Signal shutdown
    ReceiveSignal,
    /// Stop accepting
    StopAccepting,
    /// Drain sessions
    DrainSessions,
    /// Stop services
    StopServicesGraceful,
    /// Force stop services
    StopServicesForced,
    /// Stop maintenance
    StopMaintenance,
    /// Finalize shutdown
    Finalize,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ShutdownFlowSpec;

impl ShutdownFlowSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> ShutdownFlowState {
        ShutdownFlowState {
            phase: ShutdownPhase::Idle,
            accepts_stopped: false,
            sessions_drained: false,
            services_stopped: false,
            maintenance_stopped: false,
            forced_stop_attempted: false,
        }
    }

    #[allow(dead_code)]
    fn transition_state(
        &self,
        prev: &ShutdownFlowState,
        action: &ShutdownFlowAction,
    ) -> Option<ShutdownFlowState> {
        let mut candidate = *prev;
        let allowed = match action {
            ShutdownFlowAction::ReceiveSignal if matches!(prev.phase, ShutdownPhase::Idle) => {
                candidate.phase = ShutdownPhase::SignalReceived;
                true
            }
            ShutdownFlowAction::StopAccepting
                if matches!(prev.phase, ShutdownPhase::SignalReceived) =>
            {
                candidate.phase = ShutdownPhase::DrainingSessions;
                candidate.accepts_stopped = true;
                true
            }
            ShutdownFlowAction::DrainSessions
                if matches!(prev.phase, ShutdownPhase::DrainingSessions) =>
            {
                candidate.phase = ShutdownPhase::StoppingServices;
                candidate.sessions_drained = true;
                true
            }
            ShutdownFlowAction::StopServicesGraceful
                if matches!(prev.phase, ShutdownPhase::StoppingServices) =>
            {
                candidate.phase = ShutdownPhase::StoppingMaintenance;
                candidate.services_stopped = true;
                true
            }
            ShutdownFlowAction::StopServicesForced
                if matches!(prev.phase, ShutdownPhase::StoppingServices) =>
            {
                candidate.phase = ShutdownPhase::StoppingMaintenance;
                candidate.services_stopped = true;
                candidate.forced_stop_attempted = true;
                true
            }
            ShutdownFlowAction::StopMaintenance
                if matches!(prev.phase, ShutdownPhase::StoppingMaintenance) =>
            {
                candidate.maintenance_stopped = true;
                true
            }
            ShutdownFlowAction::Finalize
                if matches!(prev.phase, ShutdownPhase::StoppingMaintenance)
                    && prev.maintenance_stopped =>
            {
                candidate.phase = ShutdownPhase::Completed;
                true
            }
            _ => false,
        };
        (allowed && shutdown_flow_state_valid(&candidate)).then_some(candidate)
    }
}

#[allow(dead_code)]
fn shutdown_flow_state_valid(state: &ShutdownFlowState) -> bool {
    let completed_requires_all_flags = !matches!(state.phase, ShutdownPhase::Completed)
        || (state.accepts_stopped
            && state.sessions_drained
            && state.services_stopped
            && state.maintenance_stopped);
    let maintenance_after_services = !state.maintenance_stopped || state.services_stopped;
    let services_after_sessions = !state.services_stopped || state.sessions_drained;

    completed_requires_all_flags && maintenance_after_services && services_after_sessions
}

fn shutdown_model_cases() -> Vec<ModelCase<ShutdownFlowState, ShutdownFlowAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

#[invariant(ShutdownFlowSpec)]
fn completed_requires_all_shutdown_steps() -> BoolExpr<ShutdownFlowState> {
    nirvash_expr! { completed_requires_all_shutdown_steps(state) =>
        !matches!(state.phase, ShutdownPhase::Completed)
            || (state.accepts_stopped
                && state.sessions_drained
                && state.services_stopped
                && state.maintenance_stopped)
    }
}

#[invariant(ShutdownFlowSpec)]
fn maintenance_stops_after_services() -> BoolExpr<ShutdownFlowState> {
    nirvash_expr! { maintenance_stops_after_services(state) =>
        !state.maintenance_stopped || state.services_stopped
    }
}

#[invariant(ShutdownFlowSpec)]
fn services_stop_after_session_drain() -> BoolExpr<ShutdownFlowState> {
    nirvash_expr! { services_stop_after_session_drain(state) =>
        !state.services_stopped || state.sessions_drained
    }
}

#[property(ShutdownFlowSpec)]
fn signal_received_leads_to_accepts_stopped() -> Ltl<ShutdownFlowState, ShutdownFlowAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { signal_received(state) =>
            !matches!(state.phase, ShutdownPhase::Idle)
        }),
        Ltl::pred(nirvash_expr! { accepts_stopped(state) => state.accepts_stopped }),
    )
}

#[property(ShutdownFlowSpec)]
fn draining_leads_to_services_stopped() -> Ltl<ShutdownFlowState, ShutdownFlowAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { sessions_draining(state) =>
            matches!(state.phase, ShutdownPhase::DrainingSessions)
        }),
        Ltl::pred(nirvash_expr! { services_stopped(state) => state.services_stopped }),
    )
}

#[property(ShutdownFlowSpec)]
fn maintenance_stopping_leads_to_completed() -> Ltl<ShutdownFlowState, ShutdownFlowAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { maintenance_stopping(state) =>
            matches!(
                state.phase,
                ShutdownPhase::StoppingMaintenance | ShutdownPhase::Completed
            )
        }),
        Ltl::pred(nirvash_expr! { completed(state) =>
            matches!(state.phase, ShutdownPhase::Completed)
        }),
    )
}

#[fairness(ShutdownFlowSpec)]
fn accept_stop_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(
        nirvash_step_expr! { accept_stop_progress(prev, action, next) =>
            matches!(prev.phase, ShutdownPhase::SignalReceived)
                && matches!(action, ShutdownFlowAction::StopAccepting)
                && next.accepts_stopped
        },
    )
}

#[fairness(ShutdownFlowSpec)]
fn service_stop_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(
        nirvash_step_expr! { service_stop_progress(prev, action, next) =>
            matches!(
                prev.phase,
                ShutdownPhase::DrainingSessions | ShutdownPhase::StoppingServices
            ) && matches!(
                action,
                ShutdownFlowAction::DrainSessions
                    | ShutdownFlowAction::StopServicesGraceful
                    | ShutdownFlowAction::StopServicesForced
            ) && (next.sessions_drained || next.services_stopped)
        },
    )
}

#[fairness(ShutdownFlowSpec)]
fn maintenance_stop_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(
        nirvash_step_expr! { maintenance_stop_progress(prev, action, next) =>
            matches!(prev.phase, ShutdownPhase::StoppingMaintenance)
                && !prev.maintenance_stopped
                && matches!(action, ShutdownFlowAction::StopMaintenance)
                && next.maintenance_stopped
        },
    )
}

#[fairness(ShutdownFlowSpec)]
fn finalize_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(
        nirvash_step_expr! { finalize_progress(prev, action, next) =>
            matches!(prev.phase, ShutdownPhase::StoppingMaintenance)
                && prev.maintenance_stopped
                && matches!(action, ShutdownFlowAction::Finalize)
                && matches!(next.phase, ShutdownPhase::Completed)
        },
    )
}

#[subsystem_spec(model_cases(shutdown_model_cases))]
impl TransitionSystem for ShutdownFlowSpec {
    type State = ShutdownFlowState;
    type Action = ShutdownFlowAction;

    fn name(&self) -> &'static str {
        "shutdown_flow"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule receive_signal when matches!(action, ShutdownFlowAction::ReceiveSignal)
                && matches!(prev.phase, ShutdownPhase::Idle) => {
                set phase <= ShutdownPhase::SignalReceived;
            }

            rule stop_accepting when matches!(action, ShutdownFlowAction::StopAccepting)
                && matches!(prev.phase, ShutdownPhase::SignalReceived) => {
                set phase <= ShutdownPhase::DrainingSessions;
                set accepts_stopped <= true;
            }

            rule drain_sessions when matches!(action, ShutdownFlowAction::DrainSessions)
                && matches!(prev.phase, ShutdownPhase::DrainingSessions) => {
                set phase <= ShutdownPhase::StoppingServices;
                set sessions_drained <= true;
            }

            rule stop_services_graceful when matches!(action, ShutdownFlowAction::StopServicesGraceful)
                && matches!(prev.phase, ShutdownPhase::StoppingServices) => {
                set phase <= ShutdownPhase::StoppingMaintenance;
                set services_stopped <= true;
            }

            rule stop_services_forced when matches!(action, ShutdownFlowAction::StopServicesForced)
                && matches!(prev.phase, ShutdownPhase::StoppingServices) => {
                set phase <= ShutdownPhase::StoppingMaintenance;
                set services_stopped <= true;
                set forced_stop_attempted <= true;
            }

            rule stop_maintenance when matches!(action, ShutdownFlowAction::StopMaintenance)
                && matches!(prev.phase, ShutdownPhase::StoppingMaintenance) => {
                set maintenance_stopped <= true;
            }

            rule finalize when matches!(action, ShutdownFlowAction::Finalize)
                && matches!(prev.phase, ShutdownPhase::StoppingMaintenance)
                && prev.maintenance_stopped => {
                set phase <= ShutdownPhase::Completed;
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = ShutdownFlowSpec)]
const _: () = ();
