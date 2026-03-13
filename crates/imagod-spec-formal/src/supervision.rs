use nirvash::{BoolExpr, Fairness, Ltl, RelSet, Relation2, StepExpr};
use nirvash_lower::{FiniteModelDomain as _, FrontendSpec, ModelInstance};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain, RelationalState,
    SymbolicEncoding as FormalSymbolicEncoding, action_constraint, fairness, invariant,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program, property, subsystem_spec,
};

use crate::atoms::{RunnerAtom, ServiceAppAtom, ServiceAtom, service_runner};

#[derive(
    Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding, RelationalState,
)]
#[finite_model_domain(custom)]
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
    pub fn from_logs_summary(summary: &imagod_spec::LogsStateSummary) -> Self {
        let mut state = SupervisionSpec::new().initial_state();
        if summary.service_running {
            install_running_service(&mut state, ServiceAtom::Service0);
        }
        state
    }

    pub fn from_runtime_summary(summary: &imagod_spec::RuntimeStateSummary) -> Self {
        let mut state = SupervisionSpec::new().initial_state();
        if summary.service0_running {
            install_running_service(&mut state, ServiceAtom::Service0);
        } else if summary.service0_reaped {
            state.reaped_services.insert(ServiceAtom::Service0);
        }
        if summary.service1_running {
            install_running_service(&mut state, ServiceAtom::Service1);
        } else if summary.service1_reaped {
            state.reaped_services.insert(ServiceAtom::Service1);
        }
        state
    }

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

fn install_running_service(state: &mut SupervisionState, service: ServiceAtom) {
    state.endpoint_prepared.insert(service);
    state.registered_services.insert(service);
    state.ready_services.insert(service);
    state.running_services.insert(service);
    state.reaped_services.remove(&service);
    state
        .service_runners
        .insert(service, service_runner(service));
    state.service_apps.insert(service, ServiceAppAtom::Rpc);
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, nirvash_macros::FiniteModelDomain, ActionVocabulary,
)]
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

nirvash::finite_model_domain_spec!(
    SupervisionStateFiniteModelDomainSpec for SupervisionState,
    representatives = crate::state_domain::reachable_state_domain(&SupervisionSpec::new())
);

fn supervision_model_cases() -> Vec<ModelInstance<SupervisionState, SupervisionAction>> {
    vec![ModelInstance::default().with_check_deadlocks(false)]
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

#[action_constraint(SupervisionSpec, cases("default"))]
fn service0_only() -> StepExpr<SupervisionState, SupervisionAction> {
    nirvash_step_expr! { service0_only(_prev, action, _next) =>
        supervision_action_service(*action) == ServiceAtom::Service0
    }
}

#[invariant(SupervisionSpec)]
fn running_requires_ready_and_registered() -> BoolExpr<SupervisionState> {
    nirvash_expr! { running_requires_ready_and_registered(state) =>
        state.running_services.subset_of(&state.ready_services)
            && state.ready_services.subset_of(&state.registered_services)
            && state
                .registered_services
                .subset_of(&state.endpoint_prepared)
    }
}

#[invariant(SupervisionSpec)]
fn reaped_services_clear_runtime_membership() -> BoolExpr<SupervisionState> {
    nirvash_expr! { reaped_services_clear_runtime_membership(state) =>
        state.reaped_services.items().iter().all(|service| {
            !state.endpoint_prepared.contains(service)
                && !state.registered_services.contains(service)
                && !state.ready_services.contains(service)
                && !state.running_services.contains(service)
                && !state.stopping_services.contains(service)
                && !state.service_runners.domain().contains(service)
        })
    }
}

#[invariant(SupervisionSpec)]
fn runners_are_unique_per_service() -> BoolExpr<SupervisionState> {
    nirvash_expr! { runners_are_unique_per_service(state) =>
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
    }
}

#[property(SupervisionSpec)]
fn prepared_services_lead_to_ready() -> Ltl<SupervisionState, SupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { prepared_service_exists(state) =>
            state.endpoint_prepared.some() && state.ready_services.no()
        }),
        Ltl::pred(nirvash_expr! { ready_service_exists(state) => state.ready_services.some() }),
    )
}

#[property(SupervisionSpec)]
fn stopping_services_lead_to_reap() -> Ltl<SupervisionState, SupervisionAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { stopping_service_exists(state) =>
            state.stopping_services.some()
        }),
        Ltl::pred(nirvash_expr! { reaped_service_exists(state) => state.reaped_services.some() }),
    )
}

#[fairness(SupervisionSpec)]
fn bootstrap_progress_fairness() -> Fairness<SupervisionState, SupervisionAction> {
    Fairness::weak(
        nirvash_step_expr! { bootstrap_progress(prev, action, next) =>
            matches!(action, SupervisionAction::AdvanceBootstrap(_))
                && (rel_set_changed(&prev.registered_services, &next.registered_services)
                    || rel_set_changed(&prev.ready_services, &next.ready_services))
        },
    )
}

#[fairness(SupervisionSpec)]
fn reap_progress_fairness() -> Fairness<SupervisionState, SupervisionAction> {
    Fairness::weak(nirvash_step_expr! { reap_progress(prev, action, next) =>
        matches!(action, SupervisionAction::ReapService(_))
            && rel_set_changed(&prev.reaped_services, &next.reaped_services)
    })
}

#[subsystem_spec(model_cases(supervision_model_cases))]
impl FrontendSpec for SupervisionSpec {
    type State = SupervisionState;
    type Action = SupervisionAction;

    fn frontend_name(&self) -> &'static str {
        "supervision"
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
            rule prepare_endpoint when prepare_endpoint_target(action).is_some()
                && !prev.endpoint_prepared.contains(&prepare_endpoint_target(action)
                    .expect("prepare_endpoint guard ensures a service"))
                && !prev.registered_services.contains(&prepare_endpoint_target(action)
                    .expect("prepare_endpoint guard ensures a service"))
                && !prev.ready_services.contains(&prepare_endpoint_target(action)
                    .expect("prepare_endpoint guard ensures a service"))
                && !prev.running_services.contains(&prepare_endpoint_target(action)
                    .expect("prepare_endpoint guard ensures a service"))
                && !prev.stopping_services.contains(&prepare_endpoint_target(action)
                    .expect("prepare_endpoint guard ensures a service")) => {
                insert endpoint_prepared <= prepare_endpoint_target(action)
                    .expect("prepare_endpoint guard ensures a service");
                remove reaped_services <= prepare_endpoint_target(action)
                    .expect("prepare_endpoint guard ensures a service");
                set service_runners <= prepared_service_runners(prev, action);
                set service_apps <= prepared_service_apps(prev, action);
            }

            rule register_bootstrap when bootstrap_target(action).is_some()
                && prev.endpoint_prepared.contains(&bootstrap_target(action)
                    .expect("register_bootstrap guard ensures a service"))
                && !prev.registered_services.contains(&bootstrap_target(action)
                    .expect("register_bootstrap guard ensures a service"))
                && !prev.ready_services.contains(&bootstrap_target(action)
                    .expect("register_bootstrap guard ensures a service")) => {
                insert registered_services <= bootstrap_target(action)
                    .expect("register_bootstrap guard ensures a service");
            }

            rule mark_ready when bootstrap_target(action).is_some()
                && prev.endpoint_prepared.contains(&bootstrap_target(action)
                    .expect("mark_ready guard ensures a service"))
                && prev.registered_services.contains(&bootstrap_target(action)
                    .expect("mark_ready guard ensures a service"))
                && !prev.ready_services.contains(&bootstrap_target(action)
                    .expect("mark_ready guard ensures a service")) => {
                insert ready_services <= bootstrap_target(action)
                    .expect("mark_ready guard ensures a service");
            }

            rule start_serving when start_serving_target(action).is_some()
                && prev.ready_services.contains(&start_serving_target(action)
                    .expect("start_serving guard ensures a service"))
                && !prev.running_services.contains(&start_serving_target(action)
                    .expect("start_serving guard ensures a service"))
                && !prev.stopping_services.contains(&start_serving_target(action)
                    .expect("start_serving guard ensures a service")) => {
                insert running_services <= start_serving_target(action)
                    .expect("start_serving guard ensures a service");
            }

            rule request_stop when stop_target(action).is_some()
                && prev.running_services.contains(&stop_target(action)
                    .expect("request_stop guard ensures a service")) => {
                remove running_services <= stop_target(action)
                    .expect("request_stop guard ensures a service");
                insert stopping_services <= stop_target(action)
                    .expect("request_stop guard ensures a service");
            }

            rule reap_service when reap_target(action).is_some()
                && prev.stopping_services.contains(&reap_target(action)
                    .expect("reap_service guard ensures a service")) => {
                remove endpoint_prepared <= reap_target(action)
                    .expect("reap_service guard ensures a service");
                remove registered_services <= reap_target(action)
                    .expect("reap_service guard ensures a service");
                remove ready_services <= reap_target(action)
                    .expect("reap_service guard ensures a service");
                remove stopping_services <= reap_target(action)
                    .expect("reap_service guard ensures a service");
                insert reaped_services <= reap_target(action)
                    .expect("reap_service guard ensures a service");
                set service_runners <= reaped_service_runners(prev, action);
                set service_apps <= reaped_service_apps(prev, action);
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = SupervisionSpec)]
const _: () = ();

fn prepare_endpoint_target(action: &SupervisionAction) -> Option<ServiceAtom> {
    match action {
        SupervisionAction::PrepareEndpoint(service) => Some(*service),
        _ => None,
    }
}

fn bootstrap_target(action: &SupervisionAction) -> Option<ServiceAtom> {
    match action {
        SupervisionAction::AdvanceBootstrap(service) => Some(*service),
        _ => None,
    }
}

fn start_serving_target(action: &SupervisionAction) -> Option<ServiceAtom> {
    match action {
        SupervisionAction::StartServing(service) => Some(*service),
        _ => None,
    }
}

fn stop_target(action: &SupervisionAction) -> Option<ServiceAtom> {
    match action {
        SupervisionAction::RequestStop(service) => Some(*service),
        _ => None,
    }
}

fn reap_target(action: &SupervisionAction) -> Option<ServiceAtom> {
    match action {
        SupervisionAction::ReapService(service) => Some(*service),
        _ => None,
    }
}

fn prepared_service_runners(
    prev: &SupervisionState,
    action: &SupervisionAction,
) -> Relation2<ServiceAtom, RunnerAtom> {
    let service = prepare_endpoint_target(action)
        .expect("prepared_service_runners requires PrepareEndpoint action");
    let runner = service_runner(service);
    let mut runners = prev.service_runners.clone();
    runners.insert(service, runner);
    runners
}

fn prepared_service_apps(
    prev: &SupervisionState,
    action: &SupervisionAction,
) -> Relation2<ServiceAtom, ServiceAppAtom> {
    let service = prepare_endpoint_target(action)
        .expect("prepared_service_apps requires PrepareEndpoint action");
    let mut apps = prev.service_apps.clone();
    apps.insert(service, ServiceAppAtom::Rpc);
    apps
}

fn reaped_service_runners(
    prev: &SupervisionState,
    action: &SupervisionAction,
) -> Relation2<ServiceAtom, RunnerAtom> {
    let service = reap_target(action).expect("reaped_service_runners requires ReapService action");
    let mut runners = prev.service_runners.clone();
    for runner in RunnerAtom::bounded_domain().into_vec() {
        runners.remove(&service, &runner);
    }
    runners
}

fn reaped_service_apps(
    prev: &SupervisionState,
    action: &SupervisionAction,
) -> Relation2<ServiceAtom, ServiceAppAtom> {
    let service = reap_target(action).expect("reaped_service_apps requires ReapService action");
    let mut apps = prev.service_apps.clone();
    for app in ServiceAppAtom::bounded_domain().into_vec() {
        apps.remove(&service, &app);
    }
    apps
}

fn rel_set_changed<T>(left: &RelSet<T>, right: &RelSet<T>) -> bool
where
    T: nirvash::RelAtom + Clone + Eq + std::fmt::Debug + 'static,
{
    left.items() != right.items()
}

#[allow(dead_code)]
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

#[allow(dead_code)]
fn supervision_valid(state: &SupervisionState) -> bool {
    running_requires_ready_and_registered().eval(state)
        && reaped_services_clear_runtime_membership().eval(state)
        && runners_are_unique_per_service().eval(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash::{ModelBackend, ModelCheckConfig};
    use nirvash_check::ModelChecker;

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

    #[test]
    fn transition_program_matches_transition_function() {
        let spec = SupervisionSpec::new();
        let program = spec.transition_program().expect("transition program");
        let initial = spec.initial_state();

        assert_eq!(
            program
                .evaluate(
                    &initial,
                    &SupervisionAction::PrepareEndpoint(ServiceAtom::Service0),
                )
                .expect("evaluates"),
            transition_state(
                &initial,
                &SupervisionAction::PrepareEndpoint(ServiceAtom::Service0),
            )
        );
        assert_eq!(
            program
                .evaluate(
                    &initial,
                    &SupervisionAction::StartServing(ServiceAtom::Service0),
                )
                .expect("evaluates"),
            transition_state(
                &initial,
                &SupervisionAction::StartServing(ServiceAtom::Service0),
            )
        );
    }

    #[test]
    fn explicit_and_symbolic_backends_agree() {
        let spec = SupervisionSpec::new();
        let lowered = crate::lowered_spec(&spec);
        let explicit_snapshot = ModelChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .full_reachable_graph_snapshot()
        .expect("explicit supervision snapshot");
        let symbolic_snapshot = match ModelChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .full_reachable_graph_snapshot()
        {
            Ok(snapshot) => snapshot,
            Err(nirvash::ModelCheckError::UnsupportedConfiguration(message))
                if message.contains("symbolic backend requires") =>
            {
                return;
            }
            Err(error) => panic!("symbolic supervision snapshot: {error:?}"),
        };
        assert_eq!(symbolic_snapshot, explicit_snapshot);

        let explicit_result = ModelChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .check_all()
        .expect("explicit supervision result");
        let symbolic_result = match ModelChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .check_all()
        {
            Ok(result) => result,
            Err(nirvash::ModelCheckError::UnsupportedConfiguration(message))
                if message.contains("symbolic backend requires") =>
            {
                return;
            }
            Err(error) => panic!("symbolic supervision result: {error:?}"),
        };
        assert_eq!(symbolic_result, explicit_result);
    }
}
