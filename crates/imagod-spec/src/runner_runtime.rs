use imagod_ipc::RunnerAppType;
use nirvash_core::{
    Fairness, Ltl, ModelCase, RelAtom as _, RelSet, Signature as _, StatePredicate, StepPredicate,
    TransitionSystem,
};
use nirvash_macros::{
    ActionVocabulary, RelAtom, RelationalState, Signature as FormalSignature, fairness, invariant,
    property, subsystem_spec,
};

#[cfg(test)]
use crate::bounds::SPEC_RUNNER_APP_TYPES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerModeClass {
    Batch,
    Service,
    Network,
}

pub fn classify_runner_mode(app_type: RunnerAppType) -> RunnerModeClass {
    match app_type {
        RunnerAppType::Cli => RunnerModeClass::Batch,
        RunnerAppType::Rpc => RunnerModeClass::Service,
        RunnerAppType::Http | RunnerAppType::Socket => RunnerModeClass::Network,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum RuntimeEndpointAtom {
    HttpInbound,
    SocketInbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, RelAtom)]
enum HttpRequestAtom {
    Request0,
    Request1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum RuntimePhase {
    Idle,
    ComponentValidated,
    Serving,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum ComponentLoadClass {
    Unknown,
    Loadable,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum WasmTuningClass {
    Default,
    CustomValid,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, RelationalState)]
pub struct RunnerRuntimeState {
    listening_endpoints: RelSet<RuntimeEndpointAtom>,
    queued_http_requests: RelSet<HttpRequestAtom>,
    pub mode: Option<RunnerAppType>,
    pub phase: RuntimePhase,
    pub component: ComponentLoadClass,
    pub tuning: WasmTuningClass,
}

impl RunnerRuntimeState {
    pub fn has_http_listener(&self) -> bool {
        self.listening_endpoints
            .contains(&RuntimeEndpointAtom::HttpInbound)
    }

    pub fn has_socket_listener(&self) -> bool {
        self.listening_endpoints
            .contains(&RuntimeEndpointAtom::SocketInbound)
    }

    pub fn queued_http_request_count(&self) -> usize {
        self.queued_http_requests.cardinality()
    }

    pub fn has_queued_http_requests(&self) -> bool {
        self.queued_http_requests.some()
    }

    fn http_queue_full(&self) -> bool {
        self.queued_http_request_count() == HttpRequestAtom::rel_domain_len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature, ActionVocabulary)]
pub enum RunnerRuntimeAction {
    /// Select mode
    SelectMode(RunnerAppType),
    /// Apply default tuning
    ApplyDefaultTuning,
    /// Apply tuning
    ApplyCustomTuning,
    /// Apply invalid tuning
    ApplyInvalidTuning,
    /// Validate component
    ValidateComponentLoadable,
    /// Reject component
    ValidateComponentInvalid,
    /// Start serving
    StartServing,
    /// Enqueue HTTP request
    AcceptHttpRequest,
    /// Drain HTTP request
    DrainHttpRequest,
    /// Fail runtime
    FailRuntime,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RunnerRuntimeSpec;

impl RunnerRuntimeSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> RunnerRuntimeState {
        RunnerRuntimeState {
            listening_endpoints: RelSet::empty(),
            queued_http_requests: RelSet::empty(),
            mode: None,
            phase: RuntimePhase::Idle,
            component: ComponentLoadClass::Unknown,
            tuning: WasmTuningClass::Default,
        }
    }

    fn transition_state(
        &self,
        prev: &RunnerRuntimeState,
        action: &RunnerRuntimeAction,
    ) -> Option<RunnerRuntimeState> {
        let mut candidate = prev.clone();
        let allowed = match action {
            RunnerRuntimeAction::SelectMode(app_type)
                if prev.mode.is_none() && matches!(prev.phase, RuntimePhase::Idle) =>
            {
                candidate.mode = Some(*app_type);
                true
            }
            RunnerRuntimeAction::ApplyDefaultTuning
                if prev.mode.is_some() && matches!(prev.phase, RuntimePhase::Idle) =>
            {
                candidate.tuning = WasmTuningClass::Default;
                true
            }
            RunnerRuntimeAction::ApplyCustomTuning
                if prev.mode.is_some() && matches!(prev.phase, RuntimePhase::Idle) =>
            {
                candidate.tuning = WasmTuningClass::CustomValid;
                true
            }
            RunnerRuntimeAction::ApplyInvalidTuning
                if prev.mode.is_some() && matches!(prev.phase, RuntimePhase::Idle) =>
            {
                candidate.tuning = WasmTuningClass::Invalid;
                true
            }
            RunnerRuntimeAction::ValidateComponentLoadable
                if prev.mode.is_some()
                    && matches!(prev.phase, RuntimePhase::Idle)
                    && !matches!(prev.tuning, WasmTuningClass::Invalid) =>
            {
                candidate.component = ComponentLoadClass::Loadable;
                candidate.phase = RuntimePhase::ComponentValidated;
                true
            }
            RunnerRuntimeAction::ValidateComponentInvalid
                if prev.mode.is_some() && matches!(prev.phase, RuntimePhase::Idle) =>
            {
                candidate.component = ComponentLoadClass::Invalid;
                candidate.phase = RuntimePhase::Failed;
                true
            }
            RunnerRuntimeAction::StartServing
                if matches!(prev.phase, RuntimePhase::ComponentValidated)
                    && matches!(prev.component, ComponentLoadClass::Loadable)
                    && !matches!(prev.tuning, WasmTuningClass::Invalid) =>
            {
                candidate.phase = RuntimePhase::Serving;
                candidate.listening_endpoints = serving_endpoints(prev.mode);
                candidate.queued_http_requests = RelSet::empty();
                true
            }
            RunnerRuntimeAction::AcceptHttpRequest
                if matches!(prev.mode, Some(RunnerAppType::Http))
                    && matches!(prev.phase, RuntimePhase::Serving)
                    && prev.has_http_listener()
                    && !prev.http_queue_full() =>
            {
                let request = next_free_http_request(prev)?;
                candidate.queued_http_requests.insert(request);
                true
            }
            RunnerRuntimeAction::DrainHttpRequest
                if matches!(prev.mode, Some(RunnerAppType::Http))
                    && matches!(prev.phase, RuntimePhase::Serving)
                    && prev.has_queued_http_requests() =>
            {
                let request = first_queued_http_request(prev)?;
                candidate.queued_http_requests.remove(&request);
                true
            }
            RunnerRuntimeAction::FailRuntime
                if prev.mode.is_some() && !matches!(prev.phase, RuntimePhase::Failed) =>
            {
                candidate.phase = RuntimePhase::Failed;
                candidate.listening_endpoints = RelSet::empty();
                candidate.queued_http_requests = RelSet::empty();
                true
            }
            _ => false,
        };
        allowed.then_some(candidate)
    }
}

fn runner_runtime_model_cases() -> Vec<ModelCase<RunnerRuntimeState, RunnerRuntimeAction>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

#[invariant(RunnerRuntimeSpec)]
fn serving_requires_loadable_component() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("serving_requires_loadable_component", |state| {
        !matches!(state.phase, RuntimePhase::Serving)
            || matches!(state.component, ComponentLoadClass::Loadable)
    })
}

#[invariant(RunnerRuntimeSpec)]
fn http_queue_requires_http_mode() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("http_queue_requires_http_mode", |state| {
        !state.has_queued_http_requests()
            || (matches!(state.mode, Some(RunnerAppType::Http))
                && matches!(state.phase, RuntimePhase::Serving)
                && state.has_http_listener())
    })
}

#[invariant(RunnerRuntimeSpec)]
fn socket_listener_requires_socket_mode() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("socket_listener_requires_socket_mode", |state| {
        !state.has_socket_listener()
            || (matches!(state.mode, Some(RunnerAppType::Socket))
                && matches!(state.phase, RuntimePhase::Serving))
    })
}

#[invariant(RunnerRuntimeSpec)]
fn invalid_tuning_cannot_serve() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("invalid_tuning_cannot_serve", |state| {
        !matches!(state.tuning, WasmTuningClass::Invalid)
            || !matches!(state.phase, RuntimePhase::Serving)
    })
}

#[property(RunnerRuntimeSpec)]
fn component_validated_leads_to_serving_or_failed() -> Ltl<RunnerRuntimeState, RunnerRuntimeAction>
{
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("component_validated", |state| {
            matches!(state.phase, RuntimePhase::ComponentValidated)
        })),
        Ltl::pred(StatePredicate::new("serving_or_failed", |state| {
            matches!(state.phase, RuntimePhase::Serving | RuntimePhase::Failed)
        })),
    )
}

#[property(RunnerRuntimeSpec)]
fn http_queue_nonempty_leads_to_empty() -> Ltl<RunnerRuntimeState, RunnerRuntimeAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("http_queue_nonempty", |state| {
            state.has_queued_http_requests()
        })),
        Ltl::pred(StatePredicate::new("http_queue_empty", |state| {
            !state.has_queued_http_requests()
        })),
    )
}

#[property(RunnerRuntimeSpec)]
fn invalid_tuning_leads_to_failure() -> Ltl<RunnerRuntimeState, RunnerRuntimeAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("invalid_tuning", |state| {
            matches!(state.tuning, WasmTuningClass::Invalid)
        })),
        Ltl::pred(StatePredicate::new("runtime_failed", |state| {
            matches!(state.phase, RuntimePhase::Failed)
        })),
    )
}

#[fairness(RunnerRuntimeSpec)]
fn serve_or_fail_fairness() -> Fairness<RunnerRuntimeState, RunnerRuntimeAction> {
    Fairness::weak(StepPredicate::new("serve_or_fail", |prev, action, next| {
        matches!(prev.phase, RuntimePhase::ComponentValidated)
            && matches!(
                action,
                RunnerRuntimeAction::StartServing | RunnerRuntimeAction::FailRuntime
            )
            && matches!(next.phase, RuntimePhase::Serving | RuntimePhase::Failed)
    }))
}

#[fairness(RunnerRuntimeSpec)]
fn http_drain_fairness() -> Fairness<RunnerRuntimeState, RunnerRuntimeAction> {
    Fairness::weak(StepPredicate::new(
        "drain_http_request",
        |prev, action, next| {
            matches!(action, RunnerRuntimeAction::DrainHttpRequest)
                && matches!(prev.mode, Some(RunnerAppType::Http))
                && matches!(prev.phase, RuntimePhase::Serving)
                && prev.has_queued_http_requests()
                && next.queued_http_request_count() < prev.queued_http_request_count()
        },
    ))
}

#[fairness(RunnerRuntimeSpec)]
fn failure_fairness() -> Fairness<RunnerRuntimeState, RunnerRuntimeAction> {
    Fairness::weak(StepPredicate::new("fail_runtime", |_, action, next| {
        matches!(action, RunnerRuntimeAction::FailRuntime)
            && matches!(next.phase, RuntimePhase::Failed)
    }))
}

#[subsystem_spec(model_cases(runner_runtime_model_cases))]
impl TransitionSystem for RunnerRuntimeSpec {
    type State = RunnerRuntimeState;
    type Action = RunnerRuntimeAction;

    fn name(&self) -> &'static str {
        "runner_runtime"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.transition_state(state, action)
    }
}

#[nirvash_macros::formal_tests(spec = RunnerRuntimeSpec)]
const _: () = ();

fn serving_endpoints(mode: Option<RunnerAppType>) -> RelSet<RuntimeEndpointAtom> {
    match mode {
        Some(RunnerAppType::Http) => RelSet::from_items([RuntimeEndpointAtom::HttpInbound]),
        Some(RunnerAppType::Socket) => RelSet::from_items([RuntimeEndpointAtom::SocketInbound]),
        _ => RelSet::empty(),
    }
}

fn next_free_http_request(state: &RunnerRuntimeState) -> Option<HttpRequestAtom> {
    HttpRequestAtom::bounded_domain()
        .into_vec()
        .into_iter()
        .find(|request| !state.queued_http_requests.contains(request))
}

fn first_queued_http_request(state: &RunnerRuntimeState) -> Option<HttpRequestAtom> {
    state.queued_http_requests.items().into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_mode_classifier_covers_public_modes() {
        for app_type in SPEC_RUNNER_APP_TYPES {
            let _ = classify_runner_mode(app_type);
        }
    }

    #[test]
    fn http_serving_tracks_listener_and_queue_relations() {
        let spec = RunnerRuntimeSpec::new();
        let selected = spec
            .transition(
                &spec.initial_state(),
                &RunnerRuntimeAction::SelectMode(RunnerAppType::Http),
            )
            .expect("select mode");
        let validated = spec
            .transition(&selected, &RunnerRuntimeAction::ValidateComponentLoadable)
            .expect("validate");
        let serving = spec
            .transition(&validated, &RunnerRuntimeAction::StartServing)
            .expect("start serving");
        let queued = spec
            .transition(&serving, &RunnerRuntimeAction::AcceptHttpRequest)
            .expect("queue request");
        let drained = spec
            .transition(&queued, &RunnerRuntimeAction::DrainHttpRequest)
            .expect("drain request");

        assert!(serving.has_http_listener());
        assert_eq!(
            queued.queued_http_requests.items(),
            vec![HttpRequestAtom::Request0]
        );
        assert!(!drained.has_queued_http_requests());
    }
}
