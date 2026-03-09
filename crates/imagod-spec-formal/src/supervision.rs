use nirvash_core::{
    ActionConstraint, Fairness, Ltl, ModelCase, RelSet, Relation2, Signature as _, StatePredicate,
    StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelationalState, fairness, invariant, property, subsystem_spec,
};

use crate::atoms::{RunnerAtom, ServiceAppAtom, ServiceAtom, service_runner};

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
pub struct SupervisionState {
    endpoint_prepared: RelSet<ServiceAtom>,
    registered_services: RelSet<ServiceAtom>,
    ready_services: RelSet<ServiceAtom>,
    running_services: RelSet<ServiceAtom>,
    stopping_services: RelSet<ServiceAtom>,
    reaped_services: RelSet<ServiceAtom>,
    service_runners: Relation2<ServiceAtom, RunnerAtom>,
    service_apps: Relation2<ServiceAtom, ServiceAppAtom>,
}

impl SupervisionState {
    pub fn service_is_ready(&self, service: ServiceAtom) -> bool {
        self.ready_services.contains(&service)
    }

    pub fn service_is_running(&self, service: ServiceAtom) -> bool {
        self.running_services.contains(&service)
    }

    pub fn service_is_stopping(&self, service: ServiceAtom) -> bool {
        self.stopping_services.contains(&service)
    }

    pub fn service_is_quiescent(&self, service: ServiceAtom) -> bool {
        !self.endpoint_prepared.contains(&service)
            && !self.registered_services.contains(&service)
            && !self.ready_services.contains(&service)
            && !self.running_services.contains(&service)
            && !self.stopping_services.contains(&service)
            && !self.reaped_services.contains(&service)
            && !self.service_runners.domain().contains(&service)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, nirvash_macros::Signature, ActionVocabulary)]
pub enum SupervisionAction {
    /// Prepare a runner endpoint for one service.
    PrepareEndpoint(ServiceAtom),
    /// Advance bootstrap from prepared->registered->ready.
    AdvanceBootstrap(ServiceAtom),
    /// Mark one service as serving.
    StartServing(ServiceAtom),
    /// Request stop for one running service.
    RequestStop(ServiceAtom),
    /// Reap one stopped service.
    ReapService(ServiceAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SupervisionSpec;

impl SupervisionSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> SupervisionState {
        SupervisionState {
            endpoint_prepared: RelSet::empty(),
            registered_services: RelSet::empty(),
            ready_services: RelSet::empty(),
            running_services: RelSet::empty(),
            stopping_services: RelSet::empty(),
            reaped_services: RelSet::empty(),
            service_runners: Relation2::empty(),
            service_apps: Relation2::empty(),
        }
    }
}

fn supervision_model_cases() -> Vec<ModelCase<SupervisionState, SupervisionAction>> {
    vec![
        ModelCase::default()
            .with_check_deadlocks(false)
            .with_action_constraint(ActionConstraint::new("service0_only", |_, action, _| {
                supervision_action_service(*action) == ServiceAtom::Service0
            })),
    ]
}

fn supervision_action_service(action: SupervisionAction) -> ServiceAtom {
    match action {
        SupervisionAction::PrepareEndpoint(service)
        | SupervisionAction::AdvanceBootstrap(service)
        | SupervisionAction::StartServing(service)
        | SupervisionAction::RequestStop(service)
        | SupervisionAction::ReapService(service) => service,
    }
}

#[invariant(SupervisionSpec)]
fn running_requires_ready_and_registered() -> StatePredicate<SupervisionState> {
    StatePredicate::new("running_requires_ready_and_registered", |state| {
        state.running_services.subset_of(&state.ready_services)
            && state.ready_services.subset_of(&state.registered_services)
            && state
                .registered_services
                .subset_of(&state.endpoint_prepared)
    })
}

#[invariant(SupervisionSpec)]
fn reaped_services_clear_runtime_membership() -> StatePredicate<SupervisionState> {
    StatePredicate::new("reaped_services_clear_runtime_membership", |state| {
        state.reaped_services.items().iter().all(|service| {
            !state.endpoint_prepared.contains(service)
                && !state.registered_services.contains(service)
                && !state.ready_services.contains(service)
                && !state.running_services.contains(service)
                && !state.stopping_services.contains(service)
                && !state.service_runners.domain().contains(service)
        })
    })
}

#[invariant(SupervisionSpec)]
fn runners_are_unique_per_service() -> StatePredicate<SupervisionState> {
    StatePredicate::new("runners_are_unique_per_service", |state| {
        RunnerAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|runner| {
                state
                    .service_runners
                    .pairs()
                    .iter()
                    .filter(|(_, candidate)| candidate == &runner)
                    .count()
                    <= 1
            })
    })
}

#[property(SupervisionSpec)]
fn prepared_services_lead_to_ready() -> Ltl<SupervisionState, SupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("prepared_service_exists", |state| {
            state.endpoint_prepared.some() && state.ready_services.no()
        })),
        Ltl::pred(StatePredicate::new("ready_service_exists", |state| {
            state.ready_services.some()
        })),
    )
}

#[property(SupervisionSpec)]
fn stopping_services_lead_to_reap() -> Ltl<SupervisionState, SupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("stopping_service_exists", |state| {
            state.stopping_services.some()
        })),
        Ltl::pred(StatePredicate::new("reaped_service_exists", |state| {
            state.reaped_services.some()
        })),
    )
}

#[fairness(SupervisionSpec)]
fn bootstrap_progress_fairness() -> Fairness<SupervisionState, SupervisionAction> {
    Fairness::weak(StepPredicate::new(
        "bootstrap_progress",
        |prev, action, next| {
            matches!(action, SupervisionAction::AdvanceBootstrap(_))
                && (prev.registered_services != next.registered_services
                    || prev.ready_services != next.ready_services)
        },
    ))
}

#[fairness(SupervisionSpec)]
fn reap_progress_fairness() -> Fairness<SupervisionState, SupervisionAction> {
    Fairness::weak(StepPredicate::new("reap_progress", |prev, action, next| {
        matches!(action, SupervisionAction::ReapService(_))
            && prev.reaped_services != next.reaped_services
    }))
}

#[subsystem_spec(model_cases(supervision_model_cases))]
impl TransitionSystem for SupervisionSpec {
    type State = SupervisionState;
    type Action = SupervisionAction;

    fn name(&self) -> &'static str {
        "supervision"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, prev: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_state(prev, action)
    }
}

#[nirvash_macros::formal_tests(spec = SupervisionSpec)]
const _: () = ();

fn transition_state(
    prev: &SupervisionState,
    action: &SupervisionAction,
) -> Option<SupervisionState> {
    let mut candidate = prev.clone();
    let allowed = match action {
        SupervisionAction::PrepareEndpoint(service)
            if !prev.endpoint_prepared.contains(service)
                && !prev.registered_services.contains(service)
                && !prev.ready_services.contains(service)
                && !prev.running_services.contains(service)
                && !prev.stopping_services.contains(service) =>
        {
            let runner = service_runner(*service);
            candidate.endpoint_prepared.insert(*service);
            candidate.reaped_services.remove(service);
            candidate.service_runners.insert(*service, runner);
            candidate.service_apps.insert(*service, ServiceAppAtom::Rpc);
            true
        }
        SupervisionAction::AdvanceBootstrap(service)
            if prev.endpoint_prepared.contains(service)
                && !prev.ready_services.contains(service) =>
        {
            if prev.registered_services.contains(service) {
                candidate.ready_services.insert(*service);
            } else {
                candidate.registered_services.insert(*service);
            }
            true
        }
        SupervisionAction::StartServing(service)
            if prev.ready_services.contains(service)
                && !prev.running_services.contains(service)
                && !prev.stopping_services.contains(service) =>
        {
            candidate.running_services.insert(*service);
            true
        }
        SupervisionAction::RequestStop(service) if prev.running_services.contains(service) => {
            candidate.running_services.remove(service);
            candidate.stopping_services.insert(*service);
            true
        }
        SupervisionAction::ReapService(service) if prev.stopping_services.contains(service) => {
            candidate.endpoint_prepared.remove(service);
            candidate.registered_services.remove(service);
            candidate.ready_services.remove(service);
            candidate.stopping_services.remove(service);
            candidate.reaped_services.insert(*service);
            for runner in RunnerAtom::bounded_domain().into_vec() {
                candidate.service_runners.remove(service, &runner);
            }
            for app in ServiceAppAtom::bounded_domain().into_vec() {
                candidate.service_apps.remove(service, &app);
            }
            true
        }
        _ => false,
    };

    allowed.then_some(candidate).filter(supervision_valid)
}

fn supervision_valid(state: &SupervisionState) -> bool {
    running_requires_ready_and_registered().eval(state)
        && reaped_services_clear_runtime_membership().eval(state)
        && runners_are_unique_per_service().eval(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_lifecycle_is_scoped_per_service() {
        let spec = SupervisionSpec::new();
        let prepared = spec
            .transition(
                &spec.initial_state(),
                &SupervisionAction::PrepareEndpoint(ServiceAtom::Service0),
            )
            .expect("prepare");
        let registered = spec
            .transition(
                &prepared,
                &SupervisionAction::AdvanceBootstrap(ServiceAtom::Service0),
            )
            .expect("register");
        let ready = spec
            .transition(
                &registered,
                &SupervisionAction::AdvanceBootstrap(ServiceAtom::Service0),
            )
            .expect("ready");

        assert!(ready.service_is_ready(ServiceAtom::Service0));
        assert!(!ready.service_is_ready(ServiceAtom::Service1));
    }
}
