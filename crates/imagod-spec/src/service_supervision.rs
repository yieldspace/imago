use nirvash_core::{Fairness, Ltl, StatePredicate, StepPredicate, TransitionSystem};
use nirvash_macros::{Signature, fairness, invariant, property, subsystem_spec};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceSupervisionState {
    pub active_services: ServiceSlots,
    pub phase: ServicePhase,
    pub retained_logs: bool,
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

fn service_supervision_state_valid(state: &ServiceSupervisionState) -> bool {
    let active_matches_phase = match state.phase {
        ServicePhase::Idle | ServicePhase::Reaped => state.active_services.is_zero(),
        ServicePhase::Starting
        | ServicePhase::WaitingReady
        | ServicePhase::Running
        | ServicePhase::Stopping
        | ServicePhase::ForcedStop => !state.active_services.is_zero(),
    };
    let logs_after_reap =
        !state.retained_logs || matches!(state.phase, ServicePhase::Reaped | ServicePhase::Idle);

    active_matches_phase && logs_after_reap
}

#[invariant(ServiceSupervisionSpec)]
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

#[invariant(ServiceSupervisionSpec)]
fn reaped_clears_active_service_count() -> StatePredicate<ServiceSupervisionState> {
    StatePredicate::new("reaped_clears_active_service_count", |state| {
        !matches!(state.phase, ServicePhase::Reaped) || state.active_services.is_zero()
    })
}

#[invariant(ServiceSupervisionSpec)]
fn logs_are_only_retained_after_reap() -> StatePredicate<ServiceSupervisionState> {
    StatePredicate::new("logs_are_only_retained_after_reap", |state| {
        !state.retained_logs || matches!(state.phase, ServicePhase::Reaped | ServicePhase::Idle)
    })
}

#[property(ServiceSupervisionSpec)]
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

#[property(ServiceSupervisionSpec)]
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

#[property(ServiceSupervisionSpec)]
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

#[fairness(ServiceSupervisionSpec)]
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

#[fairness(ServiceSupervisionSpec)]
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

#[fairness(ServiceSupervisionSpec)]
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

#[subsystem_spec]
impl TransitionSystem for ServiceSupervisionSpec {
    type State = ServiceSupervisionState;
    type Action = ServiceSupervisionAction;

    fn name(&self) -> &'static str {
        "service_supervision"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        action_vocabulary()
    }

    fn transition(&self, prev: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_state(prev, action)
    }
}

#[nirvash_macros::formal_tests(spec = ServiceSupervisionSpec)]
const _: () = ();

fn action_vocabulary() -> Vec<ServiceSupervisionAction> {
    vec![
        ServiceSupervisionAction::StartService,
        ServiceSupervisionAction::RegisterRunner,
        ServiceSupervisionAction::MarkRunnerReady,
        ServiceSupervisionAction::RequestStop,
        ServiceSupervisionAction::ForceStop,
        ServiceSupervisionAction::ReapService,
        ServiceSupervisionAction::RetainLogs,
        ServiceSupervisionAction::ClearRetainedLogs,
    ]
}

fn transition_state(
    prev: &ServiceSupervisionState,
    action: &ServiceSupervisionAction,
) -> Option<ServiceSupervisionState> {
    let mut candidate = *prev;
    let allowed = match action {
        ServiceSupervisionAction::StartService
            if matches!(prev.phase, ServicePhase::Idle | ServicePhase::Reaped)
                && !prev.active_services.is_max() =>
        {
            candidate.active_services = prev.active_services.saturating_inc();
            candidate.phase = ServicePhase::Starting;
            candidate.retained_logs = false;
            true
        }
        ServiceSupervisionAction::RegisterRunner
            if matches!(prev.phase, ServicePhase::Starting) =>
        {
            candidate.phase = ServicePhase::WaitingReady;
            true
        }
        ServiceSupervisionAction::MarkRunnerReady
            if matches!(prev.phase, ServicePhase::WaitingReady) =>
        {
            candidate.phase = ServicePhase::Running;
            true
        }
        ServiceSupervisionAction::RequestStop if matches!(prev.phase, ServicePhase::Running) => {
            candidate.phase = ServicePhase::Stopping;
            true
        }
        ServiceSupervisionAction::ForceStop
            if matches!(prev.phase, ServicePhase::Running | ServicePhase::Stopping) =>
        {
            candidate.phase = ServicePhase::ForcedStop;
            true
        }
        ServiceSupervisionAction::ReapService
            if matches!(
                prev.phase,
                ServicePhase::Stopping | ServicePhase::ForcedStop
            ) =>
        {
            candidate.phase = ServicePhase::Reaped;
            candidate.active_services = ServiceSlots::new(0).expect("within bounds");
            true
        }
        ServiceSupervisionAction::RetainLogs if matches!(prev.phase, ServicePhase::Reaped) => {
            candidate.retained_logs = true;
            true
        }
        ServiceSupervisionAction::ClearRetainedLogs
            if matches!(prev.phase, ServicePhase::Reaped) && prev.retained_logs =>
        {
            candidate.phase = ServicePhase::Idle;
            candidate.retained_logs = false;
            true
        }
        _ => false,
    };

    allowed
        .then_some(candidate)
        .filter(service_supervision_state_valid)
}
