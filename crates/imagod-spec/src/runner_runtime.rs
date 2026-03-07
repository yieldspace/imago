use imago_formal_core::{
    BoundedDomain, Fairness, Ltl, Signature, StatePredicate, StepPredicate, TransitionSystem,
};
use imago_formal_macros::{
    Signature as FormalSignature, imago_fairness, imago_illegal, imago_invariant, imago_property,
    imago_subsystem_spec,
};
use imagod_ipc::RunnerAppType;

use crate::bounds::{EpochTicks, HttpQueueDepth, SPEC_RUNNER_APP_TYPES};

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
    Ready,
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
    OutboundOnly,
    Bidirectional,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
pub struct RunnerRuntimeState {
    pub mode: Option<RunnerAppType>,
    pub phase: RuntimePhase,
    pub http_queue_depth: HttpQueueDepth,
    pub epoch_ticks: EpochTicks,
    pub component: ComponentLoadClass,
    pub tuning: WasmTuningClass,
    pub socket_policy: SocketPolicyClass,
}

impl RunnerRuntimeStateSignatureSpec for RunnerRuntimeState {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            RunnerRuntimeSpec::new().initial_state(),
            Self {
                mode: Some(RunnerAppType::Http),
                phase: RuntimePhase::Serving,
                http_queue_depth: HttpQueueDepth::new(1).expect("within bounds"),
                epoch_ticks: EpochTicks::new(1).expect("within bounds"),
                component: ComponentLoadClass::Loadable,
                tuning: WasmTuningClass::Default,
                socket_policy: SocketPolicyClass::NotApplicable,
            },
            Self {
                mode: Some(RunnerAppType::Socket),
                phase: RuntimePhase::Ready,
                http_queue_depth: HttpQueueDepth::new(0).expect("within bounds"),
                epoch_ticks: EpochTicks::new(0).expect("within bounds"),
                component: ComponentLoadClass::Loadable,
                tuning: WasmTuningClass::CustomValid,
                socket_policy: SocketPolicyClass::InboundOnly,
            },
            Self {
                mode: Some(RunnerAppType::Rpc),
                phase: RuntimePhase::Failed,
                http_queue_depth: HttpQueueDepth::new(0).expect("within bounds"),
                epoch_ticks: EpochTicks::new(2).expect("within bounds"),
                component: ComponentLoadClass::Invalid,
                tuning: WasmTuningClass::Invalid,
                socket_policy: SocketPolicyClass::NotApplicable,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        let serving_requires_component = !matches!(self.phase, RuntimePhase::Serving)
            || matches!(self.component, ComponentLoadClass::Loadable);
        let http_queue_requires_http_mode = self.http_queue_depth.is_zero()
            || (matches!(self.mode, Some(RunnerAppType::Http))
                && matches!(self.phase, RuntimePhase::Serving));
        let socket_policy_matches_mode =
            matches!(self.socket_policy, SocketPolicyClass::NotApplicable)
                || matches!(self.mode, Some(RunnerAppType::Socket));
        let invalid_tuning_cannot_serve = !matches!(self.tuning, WasmTuningClass::Invalid)
            || !matches!(self.phase, RuntimePhase::Serving);

        serving_requires_component
            && http_queue_requires_http_mode
            && socket_policy_matches_mode
            && invalid_tuning_cannot_serve
    }
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
    InvokeRpc,
    RunCli,
    AcceptSocketTraffic,
    Tick,
    FailRuntime,
}

impl Signature for RunnerRuntimeAction {
    fn bounded_domain() -> BoundedDomain<Self> {
        let mut values = vec![
            Self::ApplyDefaultTuning,
            Self::ApplyCustomTuning,
            Self::ApplyInvalidTuning,
            Self::ValidateComponentLoadable,
            Self::ValidateComponentInvalid,
            Self::StartServing,
            Self::AcceptHttpRequest,
            Self::DrainHttpRequest,
            Self::InvokeRpc,
            Self::RunCli,
            Self::AcceptSocketTraffic,
            Self::Tick,
            Self::FailRuntime,
        ];
        values.extend(SPEC_RUNNER_APP_TYPES.into_iter().map(Self::SelectMode));
        BoundedDomain::new(values)
    }
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
            http_queue_depth: HttpQueueDepth::new(0).expect("within bounds"),
            epoch_ticks: EpochTicks::new(0).expect("within bounds"),
            component: ComponentLoadClass::Unknown,
            tuning: WasmTuningClass::Default,
            socket_policy: SocketPolicyClass::NotApplicable,
        }
    }
}

#[imago_invariant]
fn serving_requires_loadable_component() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("serving_requires_loadable_component", |state| {
        !matches!(state.phase, RuntimePhase::Serving)
            || matches!(state.component, ComponentLoadClass::Loadable)
    })
}

#[imago_invariant]
fn http_queue_requires_http_mode() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("http_queue_requires_http_mode", |state| {
        state.http_queue_depth.is_zero()
            || (matches!(state.mode, Some(RunnerAppType::Http))
                && matches!(state.phase, RuntimePhase::Serving))
    })
}

#[imago_invariant]
fn socket_policy_requires_socket_mode() -> StatePredicate<RunnerRuntimeState> {
    StatePredicate::new("socket_policy_requires_socket_mode", |state| {
        matches!(state.socket_policy, SocketPolicyClass::NotApplicable)
            || matches!(state.mode, Some(RunnerAppType::Socket))
    })
}

#[imago_illegal]
fn accept_http_in_non_http_mode() -> StepPredicate<RunnerRuntimeState, RunnerRuntimeAction> {
    StepPredicate::new("accept_http_in_non_http_mode", |prev, action, _| {
        matches!(action, RunnerRuntimeAction::AcceptHttpRequest)
            && !matches!(prev.mode, Some(RunnerAppType::Http))
    })
}

#[imago_illegal]
fn serve_invalid_component() -> StepPredicate<RunnerRuntimeState, RunnerRuntimeAction> {
    StepPredicate::new("serve_invalid_component", |prev, action, _| {
        matches!(action, RunnerRuntimeAction::StartServing)
            && !matches!(prev.component, ComponentLoadClass::Loadable)
    })
}

#[imago_illegal]
fn serve_with_invalid_tuning() -> StepPredicate<RunnerRuntimeState, RunnerRuntimeAction> {
    StepPredicate::new("serve_with_invalid_tuning", |prev, action, _| {
        matches!(action, RunnerRuntimeAction::StartServing)
            && matches!(prev.tuning, WasmTuningClass::Invalid)
    })
}

#[imago_property]
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

#[imago_property]
fn http_queue_full_leads_to_not_full() -> Ltl<RunnerRuntimeState, RunnerRuntimeAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("http_queue_full", |state| {
            state.http_queue_depth.is_max()
        })),
        Ltl::pred(StatePredicate::new("http_queue_not_full", |state| {
            !state.http_queue_depth.is_max()
        })),
    )
}

#[imago_property]
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

#[imago_fairness]
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

#[imago_fairness]
fn http_drain_fairness() -> Fairness<RunnerRuntimeState, RunnerRuntimeAction> {
    Fairness::weak(StepPredicate::new(
        "drain_http_request",
        |prev, action, next| {
            matches!(action, RunnerRuntimeAction::DrainHttpRequest)
                && matches!(prev.mode, Some(RunnerAppType::Http))
                && matches!(prev.phase, RuntimePhase::Serving)
                && next.http_queue_depth.get() < prev.http_queue_depth.get()
        },
    ))
}

#[imago_fairness]
fn failure_fairness() -> Fairness<RunnerRuntimeState, RunnerRuntimeAction> {
    Fairness::weak(StepPredicate::new("fail_runtime", |_, action, next| {
        matches!(action, RunnerRuntimeAction::FailRuntime)
            && matches!(next.phase, RuntimePhase::Failed)
    }))
}

#[imago_subsystem_spec(
    invariants(
        serving_requires_loadable_component,
        http_queue_requires_http_mode,
        socket_policy_requires_socket_mode
    ),
    illegal(
        accept_http_in_non_http_mode,
        serve_invalid_component,
        serve_with_invalid_tuning
    ),
    properties(
        component_validated_leads_to_serving_or_failed,
        http_queue_full_leads_to_not_full,
        invalid_tuning_leads_to_failure
    ),
    fairness(serve_or_fail_fairness, http_drain_fairness, failure_fairness)
)]
impl TransitionSystem for RunnerRuntimeSpec {
    type State = RunnerRuntimeState;
    type Action = RunnerRuntimeAction;

    fn name(&self) -> &'static str {
        "runner_runtime"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            RunnerRuntimeAction::SelectMode(app_type) if prev.mode.is_none() => {
                candidate.mode = Some(*app_type);
                candidate.socket_policy = match app_type {
                    RunnerAppType::Cli | RunnerAppType::Rpc | RunnerAppType::Http => {
                        SocketPolicyClass::NotApplicable
                    }
                    RunnerAppType::Socket => SocketPolicyClass::InboundOnly,
                };
            }
            RunnerRuntimeAction::ApplyDefaultTuning
                if !matches!(prev.phase, RuntimePhase::Serving) =>
            {
                candidate.tuning = WasmTuningClass::Default;
            }
            RunnerRuntimeAction::ApplyCustomTuning
                if !matches!(prev.phase, RuntimePhase::Serving) =>
            {
                candidate.tuning = WasmTuningClass::CustomValid;
            }
            RunnerRuntimeAction::ApplyInvalidTuning
                if !matches!(prev.phase, RuntimePhase::Serving) =>
            {
                candidate.tuning = WasmTuningClass::Invalid;
            }
            RunnerRuntimeAction::ValidateComponentLoadable
                if prev.mode.is_some() && !matches!(prev.tuning, WasmTuningClass::Invalid) =>
            {
                candidate.component = ComponentLoadClass::Loadable;
                candidate.phase = RuntimePhase::ComponentValidated;
            }
            RunnerRuntimeAction::ValidateComponentInvalid if prev.mode.is_some() => {
                candidate.component = ComponentLoadClass::Invalid;
                candidate.phase = RuntimePhase::Failed;
            }
            RunnerRuntimeAction::StartServing
                if matches!(prev.component, ComponentLoadClass::Loadable)
                    && !matches!(prev.tuning, WasmTuningClass::Invalid) =>
            {
                candidate.phase = RuntimePhase::Serving;
            }
            RunnerRuntimeAction::AcceptHttpRequest
                if matches!(prev.mode, Some(RunnerAppType::Http))
                    && matches!(prev.phase, RuntimePhase::Serving)
                    && !prev.http_queue_depth.is_max() =>
            {
                candidate.http_queue_depth = prev.http_queue_depth.saturating_inc();
            }
            RunnerRuntimeAction::DrainHttpRequest
                if matches!(prev.mode, Some(RunnerAppType::Http))
                    && matches!(prev.phase, RuntimePhase::Serving)
                    && !prev.http_queue_depth.is_zero() =>
            {
                candidate.http_queue_depth = prev.http_queue_depth.saturating_dec();
            }
            RunnerRuntimeAction::InvokeRpc
                if matches!(prev.mode, Some(RunnerAppType::Rpc))
                    && matches!(prev.phase, RuntimePhase::Serving) => {}
            RunnerRuntimeAction::RunCli
                if matches!(prev.mode, Some(RunnerAppType::Cli))
                    && matches!(prev.phase, RuntimePhase::Serving) => {}
            RunnerRuntimeAction::AcceptSocketTraffic
                if matches!(prev.mode, Some(RunnerAppType::Socket))
                    && matches!(prev.phase, RuntimePhase::Serving) => {}
            RunnerRuntimeAction::Tick if matches!(prev.phase, RuntimePhase::Serving) => {
                candidate.epoch_ticks = prev.epoch_ticks.saturating_inc();
            }
            RunnerRuntimeAction::FailRuntime if prev.mode.is_some() => {
                candidate.phase = RuntimePhase::Failed;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[cfg(test)]
#[imago_formal_macros::imago_formal_tests(spec = RunnerRuntimeSpec, init = initial_state)]
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
