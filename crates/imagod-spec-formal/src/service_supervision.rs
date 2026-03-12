use nirvash::{BoolExpr, Fairness, Ltl, RelSet, Signature as _, StepExpr, TransitionSystem};
use nirvash_macros::{
    ActionVocabulary, RelAtom, RelationalState, Signature, fairness, invariant, nirvash_expr,
    nirvash_step_expr, nirvash_transition_program, property, subsystem_spec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, RelAtom)]
enum ServiceAtom {
    Service0,
    Service1,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Signature, RelationalState)]
#[signature(custom)]
pub struct ServiceSupervisionState {
    active_services: RelSet<ServiceAtom>,
    ready_services: RelSet<ServiceAtom>,
    retained_logs: RelSet<ServiceAtom>,
    pub phase: ServicePhase,
}

impl ServiceSupervisionState {
    pub fn active_service_count(&self) -> usize {
        self.active_services.cardinality()
    }

    pub fn has_active_service(&self) -> bool {
        self.active_services.some()
    }

    pub fn has_ready_service(&self) -> bool {
        self.ready_services.some()
    }

    pub fn has_retained_logs(&self) -> bool {
        self.retained_logs.some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
pub enum ServiceSupervisionAction {
    /// Start service
    StartService,
    /// Register service
    RegisterRunner,
    /// Mark ready
    MarkRunnerReady,
    /// Stop service
    RequestStop,
    /// Force stop
    ForceStop,
    /// Reap service
    ReapService,
    /// Retain logs
    RetainLogs,
    /// Clear logs
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
            active_services: RelSet::empty(),
            ready_services: RelSet::empty(),
            retained_logs: RelSet::empty(),
            phase: ServicePhase::Idle,
        }
    }
}

nirvash::signature_spec!(
    ServiceSupervisionStateSignatureSpec for ServiceSupervisionState,
    representatives = crate::state_domain::reachable_state_domain(&ServiceSupervisionSpec::new())
);

nirvash::symbolic_state_spec!(for ServiceSupervisionState {
    active_services: RelSet<ServiceAtom>,
    ready_services: RelSet<ServiceAtom>,
    retained_logs: RelSet<ServiceAtom>,
    phase: ServicePhase,
});

fn service_supervision_state_valid(state: &ServiceSupervisionState) -> bool {
    let active_matches_phase = match state.phase {
        ServicePhase::Idle | ServicePhase::Reaped => state.active_services.no(),
        ServicePhase::Starting
        | ServicePhase::WaitingReady
        | ServicePhase::Running
        | ServicePhase::Stopping
        | ServicePhase::ForcedStop => state.active_services.some(),
    };

    let ready_matches_phase = match state.phase {
        ServicePhase::Idle | ServicePhase::Starting | ServicePhase::WaitingReady => {
            state.ready_services.no()
        }
        ServicePhase::Running | ServicePhase::Stopping | ServicePhase::ForcedStop => {
            state.ready_services.some() && state.ready_services.subset_of(&state.active_services)
        }
        ServicePhase::Reaped => state.ready_services.lone(),
    };

    let logs_after_reap = !state.has_retained_logs() || matches!(state.phase, ServicePhase::Reaped);
    let retained_subset_of_ready = state.retained_logs.subset_of(&state.ready_services);

    active_matches_phase && ready_matches_phase && logs_after_reap && retained_subset_of_ready
}

#[invariant(ServiceSupervisionSpec)]
fn running_requires_active_service() -> BoolExpr<ServiceSupervisionState> {
    nirvash_expr! { running_requires_active_service(state) =>
        !matches!(
            state.phase,
            ServicePhase::Starting
                | ServicePhase::WaitingReady
                | ServicePhase::Running
                | ServicePhase::Stopping
                | ServicePhase::ForcedStop
        ) || state.has_active_service()
    }
}

#[invariant(ServiceSupervisionSpec)]
fn reaped_clears_active_service_count() -> BoolExpr<ServiceSupervisionState> {
    nirvash_expr! { reaped_clears_active_service_count(state) =>
        !matches!(state.phase, ServicePhase::Reaped) || !state.has_active_service()
    }
}

#[invariant(ServiceSupervisionSpec)]
fn logs_are_only_retained_after_reap() -> BoolExpr<ServiceSupervisionState> {
    nirvash_expr! { logs_are_only_retained_after_reap(state) =>
        !state.has_retained_logs() || matches!(state.phase, ServicePhase::Reaped)
    }
}

#[property(ServiceSupervisionSpec)]
fn starting_leads_to_ready_or_running() -> Ltl<ServiceSupervisionState, ServiceSupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { starting(state) =>
            matches!(state.phase, ServicePhase::Starting)
        }),
        Ltl::pred(nirvash_expr! { waiting_ready_or_running(state) =>
            matches!(state.phase, ServicePhase::WaitingReady | ServicePhase::Running)
        }),
    )
}

#[property(ServiceSupervisionSpec)]
fn running_leads_to_stopping_or_reaped() -> Ltl<ServiceSupervisionState, ServiceSupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { running(state) =>
            matches!(state.phase, ServicePhase::Running)
        }),
        Ltl::pred(nirvash_expr! { stopping_or_reaped(state) =>
            matches!(
                state.phase,
                ServicePhase::Stopping | ServicePhase::ForcedStop | ServicePhase::Reaped
            )
        }),
    )
}

#[property(ServiceSupervisionSpec)]
fn retained_logs_eventually_clear() -> Ltl<ServiceSupervisionState, ServiceSupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { retained_logs(state) => state.has_retained_logs() }),
        Ltl::pred(nirvash_expr! { retained_logs_cleared(state) => !state.has_retained_logs() }),
    )
}

#[fairness(ServiceSupervisionSpec)]
fn bootstrap_progress() -> Fairness<ServiceSupervisionState, ServiceSupervisionAction> {
    Fairness::weak(
        nirvash_step_expr! { bootstrap_progress(prev, action, next) =>
            matches!(
                prev.phase,
                ServicePhase::Starting | ServicePhase::WaitingReady
            ) && matches!(
                action,
                ServiceSupervisionAction::RegisterRunner | ServiceSupervisionAction::MarkRunnerReady
            ) && matches!(
                next.phase,
                ServicePhase::WaitingReady | ServicePhase::Running
            )
        },
    )
}

#[fairness(ServiceSupervisionSpec)]
fn stop_progress() -> Fairness<ServiceSupervisionState, ServiceSupervisionAction> {
    Fairness::weak(nirvash_step_expr! { stop_progress(prev, action, next) =>
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
    })
}

#[fairness(ServiceSupervisionSpec)]
fn log_cleanup_progress() -> Fairness<ServiceSupervisionState, ServiceSupervisionAction> {
    Fairness::weak(
        nirvash_step_expr! { log_cleanup_progress(prev, action, next) =>
            matches!(prev.phase, ServicePhase::Reaped)
                && prev.has_retained_logs()
                && matches!(action, ServiceSupervisionAction::ClearRetainedLogs)
                && matches!(next.phase, ServicePhase::Idle)
        },
    )
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
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start_service when matches!(action, ServiceSupervisionAction::StartService)
                && matches!(prev.phase, ServicePhase::Idle | ServicePhase::Reaped)
                && !prev.has_active_service()
                && next_free_service(prev).is_some() => {
                insert active_services <= next_free_service(prev)
                    .expect("start_service guard ensures a free service atom");
                set ready_services <= RelSet::empty();
                set retained_logs <= RelSet::empty();
                set phase <= ServicePhase::Starting;
            }

            rule register_runner when matches!(action, ServiceSupervisionAction::RegisterRunner)
                && matches!(prev.phase, ServicePhase::Starting) => {
                set phase <= ServicePhase::WaitingReady;
            }

            rule mark_runner_ready when matches!(action, ServiceSupervisionAction::MarkRunnerReady)
                && matches!(prev.phase, ServicePhase::WaitingReady)
                && first_active_service(prev).is_some() => {
                insert ready_services <= first_active_service(prev)
                    .expect("mark_runner_ready guard ensures an active service");
                set phase <= ServicePhase::Running;
            }

            rule request_stop when matches!(action, ServiceSupervisionAction::RequestStop)
                && matches!(prev.phase, ServicePhase::Running) => {
                set phase <= ServicePhase::Stopping;
            }

            rule force_stop when matches!(action, ServiceSupervisionAction::ForceStop)
                && matches!(prev.phase, ServicePhase::Running | ServicePhase::Stopping) => {
                set phase <= ServicePhase::ForcedStop;
            }

            rule reap_service when matches!(action, ServiceSupervisionAction::ReapService)
                && matches!(prev.phase, ServicePhase::Stopping | ServicePhase::ForcedStop) => {
                set phase <= ServicePhase::Reaped;
                set active_services <= RelSet::empty();
                set retained_logs <= RelSet::empty();
            }

            rule retain_logs when matches!(action, ServiceSupervisionAction::RetainLogs)
                && matches!(prev.phase, ServicePhase::Reaped)
                && prev.ready_services.some()
                && prev.retained_logs.no() => {
                set retained_logs <= prev.ready_services.clone();
            }

            rule clear_retained_logs when matches!(action, ServiceSupervisionAction::ClearRetainedLogs)
                && matches!(prev.phase, ServicePhase::Reaped)
                && prev.has_retained_logs() => {
                set phase <= ServicePhase::Idle;
                set ready_services <= RelSet::empty();
                set retained_logs <= RelSet::empty();
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = ServiceSupervisionSpec)]
const _: () = ();

fn next_free_service(state: &ServiceSupervisionState) -> Option<ServiceAtom> {
    ServiceAtom::bounded_domain()
        .into_vec()
        .into_iter()
        .find(|service| !state.active_services.contains(service))
}

fn first_active_service(state: &ServiceSupervisionState) -> Option<ServiceAtom> {
    state.active_services.items().into_iter().next()
}

fn transition_state(
    prev: &ServiceSupervisionState,
    action: &ServiceSupervisionAction,
) -> Option<ServiceSupervisionState> {
    let mut candidate = prev.clone();
    let allowed = match action {
        ServiceSupervisionAction::StartService
            if matches!(prev.phase, ServicePhase::Idle | ServicePhase::Reaped)
                && !prev.has_active_service() =>
        {
            let service = next_free_service(prev)?;
            candidate.active_services.insert(service);
            candidate.ready_services = RelSet::empty();
            candidate.retained_logs = RelSet::empty();
            candidate.phase = ServicePhase::Starting;
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
            let service = first_active_service(prev)?;
            candidate.ready_services.insert(service);
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
            candidate.active_services = RelSet::empty();
            candidate.retained_logs = RelSet::empty();
            true
        }
        ServiceSupervisionAction::RetainLogs
            if matches!(prev.phase, ServicePhase::Reaped)
                && prev.ready_services.some()
                && prev.retained_logs.no() =>
        {
            candidate.retained_logs = prev.ready_services.clone();
            true
        }
        ServiceSupervisionAction::ClearRetainedLogs
            if matches!(prev.phase, ServicePhase::Reaped) && prev.has_retained_logs() =>
        {
            candidate.phase = ServicePhase::Idle;
            candidate.ready_services = RelSet::empty();
            candidate.retained_logs = RelSet::empty();
            true
        }
        _ => false,
    };

    allowed
        .then_some(candidate)
        .filter(service_supervision_state_valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reap_preserves_service_for_explicit_log_retention() {
        let spec = ServiceSupervisionSpec::new();
        let starting = spec
            .transition(
                &spec.initial_state(),
                &ServiceSupervisionAction::StartService,
            )
            .expect("start");
        let waiting = spec
            .transition(&starting, &ServiceSupervisionAction::RegisterRunner)
            .expect("register");
        let running = spec
            .transition(&waiting, &ServiceSupervisionAction::MarkRunnerReady)
            .expect("ready");
        let stopping = spec
            .transition(&running, &ServiceSupervisionAction::RequestStop)
            .expect("stop");
        let reaped = spec
            .transition(&stopping, &ServiceSupervisionAction::ReapService)
            .expect("reap");
        let retained = spec
            .transition(&reaped, &ServiceSupervisionAction::RetainLogs)
            .expect("retain logs");

        assert_eq!(running.ready_services.items(), vec![ServiceAtom::Service0]);
        assert!(reaped.active_services.no());
        assert_eq!(reaped.ready_services.items(), vec![ServiceAtom::Service0]);
        assert_eq!(retained.retained_logs.items(), vec![ServiceAtom::Service0]);
    }

    #[test]
    fn transition_program_matches_transition_function() {
        let spec = ServiceSupervisionSpec::new();
        let program = spec.transition_program().expect("transition program");
        let initial = spec.initial_state();

        assert_eq!(
            program
                .evaluate(&initial, &ServiceSupervisionAction::StartService)
                .expect("evaluates"),
            transition_state(&initial, &ServiceSupervisionAction::StartService)
        );
        assert_eq!(
            program
                .evaluate(&initial, &ServiceSupervisionAction::ClearRetainedLogs)
                .expect("evaluates"),
            transition_state(&initial, &ServiceSupervisionAction::ClearRetainedLogs)
        );
    }
}
