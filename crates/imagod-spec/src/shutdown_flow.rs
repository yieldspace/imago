use nirvash_core::{
    BoundedDomain, Fairness, Ltl, ModelCheckConfig, Signature as FormalSignature, StatePredicate,
    StepPredicate, TransitionSystem,
};
use nirvash_macros::{Signature, fairness, illegal, invariant, property, subsystem_spec};

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
#[signature(custom)]
pub struct ShutdownFlowState {
    pub phase: ShutdownPhase,
    pub accepts_stopped: bool,
    pub sessions_drained: bool,
    pub services_stopped: bool,
    pub maintenance_stopped: bool,
    pub forced_stop_attempted: bool,
}

impl ShutdownFlowStateSignatureSpec for ShutdownFlowState {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            ShutdownFlowSpec::new().initial_state(),
            Self {
                phase: ShutdownPhase::SignalReceived,
                accepts_stopped: false,
                sessions_drained: false,
                services_stopped: false,
                maintenance_stopped: false,
                forced_stop_attempted: false,
            },
            Self {
                phase: ShutdownPhase::DrainingSessions,
                accepts_stopped: true,
                sessions_drained: false,
                services_stopped: false,
                maintenance_stopped: false,
                forced_stop_attempted: false,
            },
            Self {
                phase: ShutdownPhase::StoppingServices,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: false,
                maintenance_stopped: false,
                forced_stop_attempted: false,
            },
            Self {
                phase: ShutdownPhase::StoppingMaintenance,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: false,
                forced_stop_attempted: false,
            },
            Self {
                phase: ShutdownPhase::StoppingMaintenance,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: false,
                forced_stop_attempted: true,
            },
            Self {
                phase: ShutdownPhase::StoppingMaintenance,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: true,
                forced_stop_attempted: false,
            },
            Self {
                phase: ShutdownPhase::StoppingMaintenance,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: true,
                forced_stop_attempted: true,
            },
            Self {
                phase: ShutdownPhase::Completed,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: true,
                forced_stop_attempted: false,
            },
            Self {
                phase: ShutdownPhase::Completed,
                accepts_stopped: true,
                sessions_drained: true,
                services_stopped: true,
                maintenance_stopped: true,
                forced_stop_attempted: true,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        let completed_requires_all_flags = !matches!(self.phase, ShutdownPhase::Completed)
            || (self.accepts_stopped
                && self.sessions_drained
                && self.services_stopped
                && self.maintenance_stopped);
        let maintenance_after_services = !self.maintenance_stopped || self.services_stopped;
        let services_after_sessions = !self.services_stopped || self.sessions_drained;

        completed_requires_all_flags && maintenance_after_services && services_after_sessions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum ShutdownFlowAction {
    ReceiveSignal,
    StopAccepting,
    DrainSessions,
    StopServicesGraceful,
    StopServicesForced,
    StopMaintenance,
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
}

fn shutdown_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        check_deadlocks: false,
        ..ModelCheckConfig::default()
    }
}

#[invariant(ShutdownFlowSpec)]
fn completed_requires_all_shutdown_steps() -> StatePredicate<ShutdownFlowState> {
    StatePredicate::new("completed_requires_all_shutdown_steps", |state| {
        !matches!(state.phase, ShutdownPhase::Completed)
            || (state.accepts_stopped
                && state.sessions_drained
                && state.services_stopped
                && state.maintenance_stopped)
    })
}

#[invariant(ShutdownFlowSpec)]
fn maintenance_stops_after_services() -> StatePredicate<ShutdownFlowState> {
    StatePredicate::new("maintenance_stops_after_services", |state| {
        !state.maintenance_stopped || state.services_stopped
    })
}

#[invariant(ShutdownFlowSpec)]
fn services_stop_after_session_drain() -> StatePredicate<ShutdownFlowState> {
    StatePredicate::new("services_stop_after_session_drain", |state| {
        !state.services_stopped || state.sessions_drained
    })
}

#[illegal(ShutdownFlowSpec)]
fn stop_accepting_before_signal() -> StepPredicate<ShutdownFlowState, ShutdownFlowAction> {
    StepPredicate::new("stop_accepting_before_signal", |prev, action, _| {
        matches!(action, ShutdownFlowAction::StopAccepting)
            && !matches!(prev.phase, ShutdownPhase::SignalReceived)
    })
}

#[illegal(ShutdownFlowSpec)]
fn stop_services_before_sessions_drained() -> StepPredicate<ShutdownFlowState, ShutdownFlowAction> {
    StepPredicate::new(
        "stop_services_before_sessions_drained",
        |prev, action, _| {
            matches!(
                action,
                ShutdownFlowAction::StopServicesGraceful | ShutdownFlowAction::StopServicesForced
            ) && !prev.sessions_drained
        },
    )
}

#[illegal(ShutdownFlowSpec)]
fn finalize_before_maintenance_stops() -> StepPredicate<ShutdownFlowState, ShutdownFlowAction> {
    StepPredicate::new("finalize_before_maintenance_stops", |prev, action, _| {
        matches!(action, ShutdownFlowAction::Finalize) && !prev.maintenance_stopped
    })
}

#[property(ShutdownFlowSpec)]
fn signal_received_leads_to_accepts_stopped() -> Ltl<ShutdownFlowState, ShutdownFlowAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("signal_received", |state| {
            !matches!(state.phase, ShutdownPhase::Idle)
        })),
        Ltl::pred(StatePredicate::new("accepts_stopped", |state| {
            state.accepts_stopped
        })),
    )
}

#[property(ShutdownFlowSpec)]
fn draining_leads_to_services_stopped() -> Ltl<ShutdownFlowState, ShutdownFlowAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("sessions_draining", |state| {
            matches!(state.phase, ShutdownPhase::DrainingSessions)
        })),
        Ltl::pred(StatePredicate::new("services_stopped", |state| {
            state.services_stopped
        })),
    )
}

#[property(ShutdownFlowSpec)]
fn maintenance_stopping_leads_to_completed() -> Ltl<ShutdownFlowState, ShutdownFlowAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("maintenance_stopping", |state| {
            matches!(
                state.phase,
                ShutdownPhase::StoppingMaintenance | ShutdownPhase::Completed
            )
        })),
        Ltl::pred(StatePredicate::new("completed", |state| {
            matches!(state.phase, ShutdownPhase::Completed)
        })),
    )
}

#[fairness(ShutdownFlowSpec)]
fn accept_stop_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(StepPredicate::new(
        "accept_stop_progress",
        |prev, action, next| {
            matches!(prev.phase, ShutdownPhase::SignalReceived)
                && matches!(action, ShutdownFlowAction::StopAccepting)
                && next.accepts_stopped
        },
    ))
}

#[fairness(ShutdownFlowSpec)]
fn service_stop_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(StepPredicate::new(
        "service_stop_progress",
        |prev, action, next| {
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
    ))
}

#[fairness(ShutdownFlowSpec)]
fn maintenance_stop_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(StepPredicate::new(
        "maintenance_stop_progress",
        |prev, action, next| {
            matches!(prev.phase, ShutdownPhase::StoppingMaintenance)
                && !prev.maintenance_stopped
                && matches!(action, ShutdownFlowAction::StopMaintenance)
                && next.maintenance_stopped
        },
    ))
}

#[fairness(ShutdownFlowSpec)]
fn finalize_progress() -> Fairness<ShutdownFlowState, ShutdownFlowAction> {
    Fairness::weak(StepPredicate::new(
        "finalize_progress",
        |prev, action, next| {
            matches!(prev.phase, ShutdownPhase::StoppingMaintenance)
                && prev.maintenance_stopped
                && matches!(action, ShutdownFlowAction::Finalize)
                && matches!(next.phase, ShutdownPhase::Completed)
        },
    ))
}

#[subsystem_spec(checker_config(shutdown_checker_config))]
impl TransitionSystem for ShutdownFlowSpec {
    type State = ShutdownFlowState;
    type Action = ShutdownFlowAction;

    fn name(&self) -> &'static str {
        "shutdown_flow"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            ShutdownFlowAction::ReceiveSignal if matches!(prev.phase, ShutdownPhase::Idle) => {
                candidate.phase = ShutdownPhase::SignalReceived;
            }
            ShutdownFlowAction::StopAccepting
                if matches!(prev.phase, ShutdownPhase::SignalReceived) =>
            {
                candidate.phase = ShutdownPhase::DrainingSessions;
                candidate.accepts_stopped = true;
            }
            ShutdownFlowAction::DrainSessions
                if matches!(prev.phase, ShutdownPhase::DrainingSessions) =>
            {
                candidate.phase = ShutdownPhase::StoppingServices;
                candidate.sessions_drained = true;
            }
            ShutdownFlowAction::StopServicesGraceful
                if matches!(prev.phase, ShutdownPhase::StoppingServices) =>
            {
                candidate.phase = ShutdownPhase::StoppingMaintenance;
                candidate.services_stopped = true;
            }
            ShutdownFlowAction::StopServicesForced
                if matches!(prev.phase, ShutdownPhase::StoppingServices) =>
            {
                candidate.phase = ShutdownPhase::StoppingMaintenance;
                candidate.services_stopped = true;
                candidate.forced_stop_attempted = true;
            }
            ShutdownFlowAction::StopMaintenance
                if matches!(prev.phase, ShutdownPhase::StoppingMaintenance) =>
            {
                candidate.maintenance_stopped = true;
            }
            ShutdownFlowAction::Finalize
                if matches!(prev.phase, ShutdownPhase::StoppingMaintenance)
                    && prev.maintenance_stopped =>
            {
                candidate.phase = ShutdownPhase::Completed;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[cfg(test)]
#[nirvash_macros::formal_tests(spec = ShutdownFlowSpec, init = initial_state)]
const _: () = ();
