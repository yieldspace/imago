use imagod_ipc::RunnerAppType;
use nirvash_core::{Fairness, Ltl, ModelCase, StatePredicate, StepPredicate, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, fairness, invariant, property, subsystem_spec};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum SocketPolicyClass {
    NotApplicable,
    InboundOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum HttpQueueClass {
    Empty,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerRuntimeState {
    pub mode: Option<RunnerAppType>,
    pub phase: RuntimePhase,
    pub http_queue: HttpQueueClass,
    pub component: ComponentLoadClass,
    pub tuning: WasmTuningClass,
    pub socket_policy: SocketPolicyClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerRuntimeAction {
    SelectMode(RunnerAppType),
    ApplyDefaultTuning,
    ApplyCustomTuning,
    ApplyInvalidTuning,
    ValidateComponentLoadable,
    ValidateComponentInvalid,
    StartServing,
    AcceptHttpRequest,
    DrainHttpRequest,
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
            mode: None,
            phase: RuntimePhase::Idle,
            http_queue: HttpQueueClass::Empty,
            component: ComponentLoadClass::Unknown,
            tuning: WasmTuningClass::Default,
            socket_policy: SocketPolicyClass::NotApplicable,
        }
    }

    fn action_vocabulary(&self) -> Vec<RunnerRuntimeAction> {
        vec![
            RunnerRuntimeAction::SelectMode(RunnerAppType::Cli),
            RunnerRuntimeAction::SelectMode(RunnerAppType::Rpc),
            RunnerRuntimeAction::SelectMode(RunnerAppType::Http),
            RunnerRuntimeAction::SelectMode(RunnerAppType::Socket),
            RunnerRuntimeAction::ApplyDefaultTuning,
            RunnerRuntimeAction::ApplyCustomTuning,
            RunnerRuntimeAction::ApplyInvalidTuning,
            RunnerRuntimeAction::ValidateComponentLoadable,
            RunnerRuntimeAction::ValidateComponentInvalid,
            RunnerRuntimeAction::StartServing,
            RunnerRuntimeAction::AcceptHttpRequest,
            RunnerRuntimeAction::DrainHttpRequest,
            RunnerRuntimeAction::FailRuntime,
        ]
    }

    fn transition_state(
        &self,
        prev: &RunnerRuntimeState,
        action: &RunnerRuntimeAction,
    ) -> Option<RunnerRuntimeState> {
        let mut candidate = *prev;
        let allowed = match action {
            RunnerRuntimeAction::SelectMode(app_type)
                if prev.mode.is_none() && matches!(prev.phase, RuntimePhase::Idle) =>
            {
                candidate.mode = Some(*app_type);
                candidate.socket_policy = match app_type {
                    RunnerAppType::Cli | RunnerAppType::Rpc | RunnerAppType::Http => {
                        SocketPolicyClass::NotApplicable
                    }
                    RunnerAppType::Socket => SocketPolicyClass::InboundOnly,
                };
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
                candidate.http_queue = HttpQueueClass::Empty;
                true
            }
            RunnerRuntimeAction::AcceptHttpRequest
                if matches!(prev.mode, Some(RunnerAppType::Http))
                    && matches!(prev.phase, RuntimePhase::Serving)
                    && matches!(prev.http_queue, HttpQueueClass::Empty) =>
            {
                candidate.http_queue = HttpQueueClass::Full;
                true
            }
            RunnerRuntimeAction::DrainHttpRequest
                if matches!(prev.mode, Some(RunnerAppType::Http))
                    && matches!(prev.phase, RuntimePhase::Serving)
                    && matches!(prev.http_queue, HttpQueueClass::Full) =>
            {
                candidate.http_queue = HttpQueueClass::Empty;
                true
            }
            RunnerRuntimeAction::FailRuntime
                if prev.mode.is_some() && !matches!(prev.phase, RuntimePhase::Failed) =>
            {
                candidate.phase = RuntimePhase::Failed;
                candidate.http_queue = HttpQueueClass::Empty;
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
        matches!(state.http_queue, HttpQueueClass::Empty)
            || (matches!(state.mode, Some(RunnerAppType::Http))
                && matches!(state.phase, RuntimePhase::Serving))
    })
}

#[invariant(RunnerRuntimeSpec)]
fn socket_policy_requires_socket_mode() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("socket_policy_requires_socket_mode", |state| {
        matches!(state.socket_policy, SocketPolicyClass::NotApplicable)
            || matches!(state.mode, Some(RunnerAppType::Socket))
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
fn http_queue_full_leads_to_not_full() -> Ltl<RunnerRuntimeState, RunnerRuntimeAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("http_queue_full", |state| {
            matches!(state.http_queue, HttpQueueClass::Full)
        })),
        Ltl::pred(StatePredicate::new("http_queue_not_full", |state| {
            matches!(state.http_queue, HttpQueueClass::Empty)
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
                && matches!(prev.http_queue, HttpQueueClass::Full)
                && matches!(next.http_queue, HttpQueueClass::Empty)
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
        self.action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.transition_state(state, action)
    }
}

#[nirvash_macros::formal_tests(spec = RunnerRuntimeSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_mode_classifier_covers_public_modes() {
        for app_type in SPEC_RUNNER_APP_TYPES {
            let _ = classify_runner_mode(app_type);
        }
    }
}
