use imago_formal_core::{
    BoundedDomain, Fairness, Ltl, Signature as FormalSignature, StatePredicate, StepPredicate,
    TransitionSystem,
};
use imago_formal_macros::{
    Signature, imago_fairness, imago_illegal, imago_invariant, imago_property, imago_subsystem_spec,
};

use crate::bounds::ServiceSlots;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum ServicePhase {
    Idle,
    Starting,
    WaitingReady,
    Running,
    Stopping,
    ForcedStop,
    Reaped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
#[signature(custom)]
pub struct ServiceSupervisionState {
    pub active_services: ServiceSlots,
    pub phase: ServicePhase,
    pub retained_logs: bool,
}

impl ServiceSupervisionStateSignatureSpec for ServiceSupervisionState {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            ServiceSupervisionSpec::new().initial_state(),
            Self {
                active_services: ServiceSlots::new(1).expect("within bounds"),
                phase: ServicePhase::Starting,
                retained_logs: false,
            },
            Self {
                active_services: ServiceSlots::new(1).expect("within bounds"),
                phase: ServicePhase::Running,
                retained_logs: false,
            },
            Self {
                active_services: ServiceSlots::new(1).expect("within bounds"),
                phase: ServicePhase::Stopping,
                retained_logs: false,
            },
            Self {
                active_services: ServiceSlots::new(1).expect("within bounds"),
                phase: ServicePhase::ForcedStop,
                retained_logs: false,
            },
            Self {
                active_services: ServiceSlots::new(1).expect("within bounds"),
                phase: ServicePhase::WaitingReady,
                retained_logs: false,
            },
            Self {
                active_services: ServiceSlots::new(0).expect("within bounds"),
                phase: ServicePhase::Reaped,
                retained_logs: false,
            },
            Self {
                active_services: ServiceSlots::new(0).expect("within bounds"),
                phase: ServicePhase::Reaped,
                retained_logs: true,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        let active_matches_phase = match self.phase {
            ServicePhase::Idle | ServicePhase::Reaped => self.active_services.is_zero(),
            ServicePhase::Starting
            | ServicePhase::WaitingReady
            | ServicePhase::Running
            | ServicePhase::Stopping
            | ServicePhase::ForcedStop => !self.active_services.is_zero(),
        };
        let logs_after_reap =
            !self.retained_logs || matches!(self.phase, ServicePhase::Reaped | ServicePhase::Idle);

        active_matches_phase && logs_after_reap
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature)]
pub enum ServiceSupervisionAction {
    StartService,
    RegisterRunner,
    MarkRunnerReady,
    RequestStop,
    ForceStop,
    ReapService,
    RetainLogs,
    ClearRetainedLogs,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ServiceSupervisionSpec;

impl ServiceSupervisionSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> ServiceSupervisionState {
        ServiceSupervisionState {
            active_services: ServiceSlots::new(0).expect("within bounds"),
            phase: ServicePhase::Idle,
            retained_logs: false,
        }
    }
}

#[imago_invariant]
fn running_requires_active_service() -> StatePredicate<ServiceSupervisionState> {
    StatePredicate::new("running_requires_active_service", |state| {
        !matches!(
            state.phase,
            ServicePhase::Starting
                | ServicePhase::WaitingReady
                | ServicePhase::Running
                | ServicePhase::Stopping
                | ServicePhase::ForcedStop
        ) || !state.active_services.is_zero()
    })
}

#[imago_invariant]
fn reaped_clears_active_service_count() -> StatePredicate<ServiceSupervisionState> {
    StatePredicate::new("reaped_clears_active_service_count", |state| {
        !matches!(state.phase, ServicePhase::Reaped) || state.active_services.is_zero()
    })
}

#[imago_invariant]
fn logs_are_only_retained_after_reap() -> StatePredicate<ServiceSupervisionState> {
    StatePredicate::new("logs_are_only_retained_after_reap", |state| {
        !state.retained_logs || matches!(state.phase, ServicePhase::Reaped | ServicePhase::Idle)
    })
}

#[imago_illegal]
fn ready_without_registration() -> StepPredicate<ServiceSupervisionState, ServiceSupervisionAction>
{
    StepPredicate::new("ready_without_registration", |prev, action, _| {
        matches!(action, ServiceSupervisionAction::MarkRunnerReady)
            && !matches!(prev.phase, ServicePhase::WaitingReady)
    })
}

#[imago_illegal]
fn reap_without_stop() -> StepPredicate<ServiceSupervisionState, ServiceSupervisionAction> {
    StepPredicate::new("reap_without_stop", |prev, action, _| {
        matches!(action, ServiceSupervisionAction::ReapService)
            && !matches!(
                prev.phase,
                ServicePhase::Stopping | ServicePhase::ForcedStop
            )
    })
}

#[imago_illegal]
fn clear_logs_before_reap() -> StepPredicate<ServiceSupervisionState, ServiceSupervisionAction> {
    StepPredicate::new("clear_logs_before_reap", |prev, action, _| {
        matches!(action, ServiceSupervisionAction::ClearRetainedLogs)
            && !matches!(prev.phase, ServicePhase::Reaped)
    })
}

#[imago_property]
fn starting_leads_to_ready_or_running() -> Ltl<ServiceSupervisionState, ServiceSupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("starting", |state| {
            matches!(state.phase, ServicePhase::Starting)
        })),
        Ltl::pred(StatePredicate::new("waiting_ready_or_running", |state| {
            matches!(
                state.phase,
                ServicePhase::WaitingReady | ServicePhase::Running
            )
        })),
    )
}

#[imago_property]
fn running_leads_to_stopping_or_reaped() -> Ltl<ServiceSupervisionState, ServiceSupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("running", |state| {
            matches!(state.phase, ServicePhase::Running)
        })),
        Ltl::pred(StatePredicate::new("stopping_or_reaped", |state| {
            matches!(
                state.phase,
                ServicePhase::Stopping | ServicePhase::ForcedStop | ServicePhase::Reaped
            )
        })),
    )
}

#[imago_property]
fn retained_logs_eventually_clear() -> Ltl<ServiceSupervisionState, ServiceSupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("retained_logs", |state| {
            state.retained_logs
        })),
        Ltl::pred(StatePredicate::new("retained_logs_cleared", |state| {
            !state.retained_logs
        })),
    )
}

#[imago_fairness]
fn bootstrap_progress() -> Fairness<ServiceSupervisionState, ServiceSupervisionAction> {
    Fairness::weak(StepPredicate::new(
        "bootstrap_progress",
        |prev, action, next| {
            matches!(
                prev.phase,
                ServicePhase::Starting | ServicePhase::WaitingReady
            ) && matches!(
                action,
                ServiceSupervisionAction::RegisterRunner
                    | ServiceSupervisionAction::MarkRunnerReady
            ) && matches!(
                next.phase,
                ServicePhase::WaitingReady | ServicePhase::Running
            )
        },
    ))
}

#[imago_fairness]
fn stop_progress() -> Fairness<ServiceSupervisionState, ServiceSupervisionAction> {
    Fairness::weak(StepPredicate::new("stop_progress", |prev, action, next| {
        matches!(prev.phase, ServicePhase::Running | ServicePhase::Stopping)
            && matches!(
                action,
                ServiceSupervisionAction::RequestStop
                    | ServiceSupervisionAction::ForceStop
                    | ServiceSupervisionAction::ReapService
            )
            && matches!(
                next.phase,
                ServicePhase::Stopping | ServicePhase::ForcedStop | ServicePhase::Reaped
            )
    }))
}

#[imago_fairness]
fn log_cleanup_progress() -> Fairness<ServiceSupervisionState, ServiceSupervisionAction> {
    Fairness::weak(StepPredicate::new(
        "log_cleanup_progress",
        |prev, action, next| {
            matches!(prev.phase, ServicePhase::Reaped)
                && prev.retained_logs
                && matches!(action, ServiceSupervisionAction::ClearRetainedLogs)
                && matches!(next.phase, ServicePhase::Idle)
        },
    ))
}

#[imago_subsystem_spec(
    invariants(
        running_requires_active_service,
        reaped_clears_active_service_count,
        logs_are_only_retained_after_reap
    ),
    illegal(ready_without_registration, reap_without_stop, clear_logs_before_reap),
    properties(
        starting_leads_to_ready_or_running,
        running_leads_to_stopping_or_reaped,
        retained_logs_eventually_clear
    ),
    fairness(bootstrap_progress, stop_progress, log_cleanup_progress)
)]
impl TransitionSystem for ServiceSupervisionSpec {
    type State = ServiceSupervisionState;
    type Action = ServiceSupervisionAction;

    fn name(&self) -> &'static str {
        "service_supervision"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            ServiceSupervisionAction::StartService
                if matches!(prev.phase, ServicePhase::Idle | ServicePhase::Reaped)
                    && !prev.active_services.is_max() =>
            {
                candidate.active_services = prev.active_services.saturating_inc();
                candidate.phase = ServicePhase::Starting;
                candidate.retained_logs = false;
            }
            ServiceSupervisionAction::RegisterRunner
                if matches!(prev.phase, ServicePhase::Starting) =>
            {
                candidate.phase = ServicePhase::WaitingReady;
            }
            ServiceSupervisionAction::MarkRunnerReady
                if matches!(prev.phase, ServicePhase::WaitingReady) =>
            {
                candidate.phase = ServicePhase::Running;
            }
            ServiceSupervisionAction::RequestStop
                if matches!(prev.phase, ServicePhase::Running) =>
            {
                candidate.phase = ServicePhase::Stopping;
            }
            ServiceSupervisionAction::ForceStop
                if matches!(prev.phase, ServicePhase::Running | ServicePhase::Stopping) =>
            {
                candidate.phase = ServicePhase::ForcedStop;
            }
            ServiceSupervisionAction::ReapService
                if matches!(
                    prev.phase,
                    ServicePhase::Stopping | ServicePhase::ForcedStop
                ) =>
            {
                candidate.phase = ServicePhase::Reaped;
                candidate.active_services = ServiceSlots::new(0).expect("within bounds");
            }
            ServiceSupervisionAction::RetainLogs if matches!(prev.phase, ServicePhase::Reaped) => {
                candidate.retained_logs = true;
            }
            ServiceSupervisionAction::ClearRetainedLogs
                if matches!(prev.phase, ServicePhase::Reaped) && prev.retained_logs =>
            {
                candidate.phase = ServicePhase::Idle;
                candidate.retained_logs = false;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[cfg(test)]
#[imago_formal_macros::imago_formal_tests(spec = ServiceSupervisionSpec, init = initial_state)]
const _: () = ();
