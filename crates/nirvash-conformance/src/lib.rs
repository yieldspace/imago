use std::{fmt::Debug, panic::AssertUnwindSafe, process};

use nirvash::{IntoBoundedDomain, into_bounded_domain};
pub use nirvash::{ReachableGraphSnapshot, inventory};
use nirvash_lower::{FiniteModelDomain, LoweringCx, ModelBackend, ModelCheckConfig, TemporalSpec};
pub use nirvash_lower::{FrontendSpec, ModelInstance, Trace, TraceStep};

#[allow(async_fn_in_trait)]
pub trait ActionApplier {
    type Action;
    type Output;
    type Context;

    async fn execute_action(&self, context: &Self::Context, action: &Self::Action) -> Self::Output;
}

#[allow(async_fn_in_trait)]
pub trait StateObserver {
    type SummaryState;
    type Context;

    async fn observe_state(&self, context: &Self::Context) -> Self::SummaryState;
}

/// Spec-side contract for replaying runtime behavior against a lowered frontend spec.
pub trait ProtocolConformanceSpec: FrontendSpec {
    type ExpectedOutput: Clone + Debug + PartialEq + Eq;
    type ProbeState: Clone + Debug;
    type ProbeOutput: Clone + Debug;
    type SummaryState: Clone + Debug;
    type SummaryOutput: Clone + Debug;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput;

    fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState;

    fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput;

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State;

    fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput;
}

/// Binding between a spec and a concrete runtime implementation.
#[allow(async_fn_in_trait)]
pub trait ProtocolRuntimeBinding<Spec>
where
    Spec: ProtocolConformanceSpec,
{
    type Runtime: ActionApplier<Action = Spec::Action, Output = Spec::ProbeOutput, Context = Self::Context>
        + StateObserver<SummaryState = Spec::ProbeState, Context = Self::Context>;
    type Context: Clone;

    async fn fresh_runtime(spec: &Spec) -> Self::Runtime;

    fn context(spec: &Spec) -> Self::Context;
}

/// Maps concrete runtime observations into abstract states and actions for relation-based refinement.
pub trait RefinementMap<Spec: FrontendSpec> {
    type ImplState;
    type ImplInput;
    type ImplOutput;
    type AuxState;

    fn abstract_state(&self, state: &Self::ImplState, aux: &Self::AuxState) -> Spec::State;

    fn candidate_actions(
        &self,
        before: &Self::ImplState,
        input: &Self::ImplInput,
        output: &Self::ImplOutput,
        after: &Self::ImplState,
        aux: &Self::AuxState,
    ) -> Vec<Spec::Action>;
}

pub trait TraceRefinementMap<Spec: FrontendSpec> {
    type ImplState;
    type ImplInput;
    type ImplOutput;
    type AuxState: Clone;

    fn init_aux(&self, initial: &Self::ImplState) -> Self::AuxState;

    fn next_aux(
        &self,
        before: &Self::ImplState,
        event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        after: &Self::ImplState,
        aux: &Self::AuxState,
    ) -> Self::AuxState;

    fn abstract_state(&self, state: &Self::ImplState, aux: &Self::AuxState) -> Spec::State;

    fn candidate_actions(
        &self,
        before: &Self::ImplState,
        event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        after: &Self::ImplState,
        aux: &Self::AuxState,
    ) -> Vec<Spec::Action>;

    fn output_matches(
        &self,
        spec: &Spec,
        action: &Spec::Action,
        abstract_before: &Spec::State,
        abstract_after: &Spec::State,
        event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        aux: &Self::AuxState,
    ) -> bool;

    fn hidden_step(
        &self,
        _before: &Self::ImplState,
        _event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        _after: &Self::ImplState,
        _aux: &Self::AuxState,
    ) -> bool {
        false
    }
}

/// Successful relation-based refinement step witness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRefinementWitness<S, A> {
    pub abstract_before: S,
    pub chosen_action: A,
    pub abstract_after: S,
}

/// Relation-based step refinement failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepRefinementError<S, A> {
    NoCandidateActions {
        abstract_before: S,
        abstract_after: S,
    },
    NoMatchingAbstractSuccessor {
        abstract_before: S,
        abstract_after: S,
        candidate_actions: Vec<A>,
    },
}

impl<S, A> std::fmt::Display for StepRefinementError<S, A>
where
    S: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCandidateActions {
                abstract_before,
                abstract_after,
            } => write!(
                f,
                "no candidate abstract actions for concrete transition {:?} -> {:?}",
                abstract_before, abstract_after
            ),
            Self::NoMatchingAbstractSuccessor {
                abstract_before,
                abstract_after,
                candidate_actions,
            } => write!(
                f,
                "abstract state {:?} does not reach {:?} under candidate actions {:?}",
                abstract_before, abstract_after, candidate_actions
            ),
        }
    }
}

impl<S, A> std::error::Error for StepRefinementError<S, A>
where
    S: Debug,
    A: Debug,
{
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedEvent<A, Output, Input = ()> {
    Invoke { input: Input },
    Return { output: Output },
    Action { action: A, output: Output },
    Internal,
    Stutter,
}

impl<A, Output, Input> ObservedEvent<A, Output, Input> {
    fn matches_step(&self, step: &TraceStep<A>) -> bool
    where
        A: PartialEq,
    {
        match (self, step) {
            (Self::Action { action, .. }, TraceStep::Action(expected)) => action == expected,
            (Self::Stutter, TraceStep::Stutter) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateObservation<S> {
    Full(S),
    Partial(S),
    Unknown,
}

impl<S> StateObservation<S> {
    pub fn as_ref(&self) -> Option<&S> {
        match self {
            Self::Full(state) | Self::Partial(state) => Some(state),
            Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceRefinementEngine {
    ExplicitCandidate,
    ExplicitConstrained,
    SymbolicConstrained,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRefinementConfig {
    pub engine: TraceRefinementEngine,
    pub max_hidden_steps_between_observations: usize,
    pub require_total_observation: bool,
    pub allow_lasso: bool,
}

impl Default for TraceRefinementConfig {
    fn default() -> Self {
        Self {
            engine: TraceRefinementEngine::ExplicitCandidate,
            max_hidden_steps_between_observations: 0,
            require_total_observation: true,
            allow_lasso: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedTrace<SummaryState, A, SummaryOutput, SummaryInput = ()> {
    states: Vec<StateObservation<SummaryState>>,
    events: Vec<ObservedEvent<A, SummaryOutput, SummaryInput>>,
    loop_start: Option<usize>,
}

impl<SummaryState, A, SummaryOutput, SummaryInput>
    ObservedTrace<SummaryState, A, SummaryOutput, SummaryInput>
{
    pub fn new(
        states: Vec<StateObservation<SummaryState>>,
        events: Vec<ObservedEvent<A, SummaryOutput, SummaryInput>>,
        loop_start: Option<usize>,
    ) -> Self {
        Self {
            states,
            events,
            loop_start,
        }
    }

    pub fn terminal(states: Vec<SummaryState>, action_events: Vec<(A, SummaryOutput)>) -> Self {
        assert!(
            !states.is_empty(),
            "observed terminal traces require at least one state"
        );
        assert_eq!(
            states.len(),
            action_events.len() + 1,
            "observed terminal traces require exactly one more state than action events",
        );
        let mut events = action_events
            .into_iter()
            .map(|(action, output)| ObservedEvent::Action { action, output })
            .collect::<Vec<_>>();
        events.push(ObservedEvent::Stutter);
        let loop_start = Some(states.len() - 1);
        Self::new(
            states.into_iter().map(StateObservation::Full).collect(),
            events,
            loop_start,
        )
    }

    pub fn states(&self) -> &[StateObservation<SummaryState>] {
        &self.states
    }

    pub fn events(&self) -> &[ObservedEvent<A, SummaryOutput, SummaryInput>] {
        &self.events
    }

    pub const fn loop_start(&self) -> Option<usize> {
        self.loop_start
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn is_total_observation(&self) -> bool {
        self.states
            .iter()
            .all(|state| matches!(state, StateObservation::Full(_)))
    }

    pub fn observed_state(&self, index: usize) -> Option<&SummaryState> {
        self.states.get(index).and_then(StateObservation::as_ref)
    }

    pub fn next_index(&self, index: usize) -> usize {
        if index + 1 < self.states.len() {
            index + 1
        } else {
            self.loop_start.unwrap_or(index)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceStepRefinementWitness<S, A> {
    pub index: usize,
    pub abstract_before: S,
    pub step: TraceStep<A>,
    pub abstract_after: S,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRefinementWitness<S, A> {
    pub abstract_trace: Trace<S, A>,
    pub steps: Vec<TraceStepRefinementWitness<S, A>>,
    pub model_case_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceRefinementError<S, A> {
    InitialStateMismatch {
        model_case_label: String,
        observed_initial: S,
        abstract_initial: S,
        candidate_trace: Option<Trace<S, A>>,
    },
    ShapeMismatch {
        model_case_label: String,
        detail: String,
        candidate_trace: Option<Trace<S, A>>,
    },
    StepMismatch {
        model_case_label: String,
        index: usize,
        matching_prefix_len: usize,
        step: TraceStep<A>,
        abstract_before: S,
        abstract_after: S,
        expected_after: S,
        detail: String,
        refinement_error: Option<StepRefinementError<S, A>>,
        candidate_trace: Option<Trace<S, A>>,
    },
    StutterMismatch {
        model_case_label: String,
        index: usize,
        matching_prefix_len: usize,
        abstract_before: S,
        abstract_after: S,
        detail: String,
        candidate_trace: Option<Trace<S, A>>,
    },
    SearchFailed {
        model_case_label: String,
        detail: String,
    },
    NoMatchingExplicitCandidate {
        model_case_label: String,
        matching_prefix_len: usize,
        candidate_trace: Option<Trace<S, A>>,
        failure: Box<TraceRefinementError<S, A>>,
    },
}

impl<S, A> TraceRefinementError<S, A> {
    fn matching_prefix_len(&self) -> usize {
        match self {
            Self::InitialStateMismatch { .. }
            | Self::ShapeMismatch { .. }
            | Self::SearchFailed { .. } => 0,
            Self::StepMismatch {
                matching_prefix_len,
                ..
            }
            | Self::StutterMismatch {
                matching_prefix_len,
                ..
            }
            | Self::NoMatchingExplicitCandidate {
                matching_prefix_len,
                ..
            } => *matching_prefix_len,
        }
    }

    fn candidate_trace(&self) -> Option<&Trace<S, A>> {
        match self {
            Self::InitialStateMismatch {
                candidate_trace, ..
            }
            | Self::ShapeMismatch {
                candidate_trace, ..
            }
            | Self::StepMismatch {
                candidate_trace, ..
            }
            | Self::StutterMismatch {
                candidate_trace, ..
            }
            | Self::NoMatchingExplicitCandidate {
                candidate_trace, ..
            } => candidate_trace.as_ref(),
            Self::SearchFailed { .. } => None,
        }
    }

    fn failing_index(&self) -> Option<usize> {
        match self {
            Self::StepMismatch { index, .. } | Self::StutterMismatch { index, .. } => Some(*index),
            Self::NoMatchingExplicitCandidate { failure, .. } => failure.failing_index(),
            _ => None,
        }
    }
}

impl<S, A> std::fmt::Display for TraceRefinementError<S, A>
where
    S: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InitialStateMismatch {
                model_case_label,
                observed_initial,
                abstract_initial,
                candidate_trace,
            } => write!(
                f,
                "model case `{model_case_label}` initial state mismatch: observed {:?}, candidate {:?}, candidate trace {:?}",
                observed_initial, abstract_initial, candidate_trace
            ),
            Self::ShapeMismatch {
                model_case_label,
                detail,
                candidate_trace,
            } => write!(
                f,
                "model case `{model_case_label}` trace shape mismatch: {detail}; candidate trace {:?}",
                candidate_trace
            ),
            Self::StepMismatch {
                model_case_label,
                index,
                matching_prefix_len,
                step,
                abstract_before,
                abstract_after,
                expected_after,
                detail,
                refinement_error,
                candidate_trace,
            } => write!(
                f,
                "model case `{model_case_label}` failed at step {index} after matching {matching_prefix_len} steps: candidate step {:?} from {:?} expected {:?}, observed {:?}; {}; refinement error: {:?}; candidate trace {:?}",
                step,
                abstract_before,
                expected_after,
                abstract_after,
                detail,
                refinement_error,
                candidate_trace
            ),
            Self::StutterMismatch {
                model_case_label,
                index,
                matching_prefix_len,
                abstract_before,
                abstract_after,
                detail,
                candidate_trace,
            } => write!(
                f,
                "model case `{model_case_label}` stutter mismatch at step {index} after matching {matching_prefix_len} steps: before {:?}, after {:?}; {}; candidate trace {:?}",
                abstract_before, abstract_after, detail, candidate_trace
            ),
            Self::SearchFailed {
                model_case_label,
                detail,
            } => write!(
                f,
                "model case `{model_case_label}` candidate search failed: {detail}"
            ),
            Self::NoMatchingExplicitCandidate {
                model_case_label,
                matching_prefix_len,
                candidate_trace,
                failure,
            } => write!(
                f,
                "model case `{model_case_label}` found no matching explicit candidate after matching {matching_prefix_len} steps; failing index {:?}; candidate trace {:?}; cause: {}",
                failure.failing_index(),
                candidate_trace,
                failure
            ),
        }
    }
}

impl<S, A> std::error::Error for TraceRefinementError<S, A>
where
    S: Debug,
    A: Debug,
{
}

/// Concrete input that should follow a valid abstract transition.
#[derive(Debug, Clone)]
pub struct PositiveWitness<Context, Input> {
    name: String,
    context: Context,
    input: Input,
    canonical: bool,
}

impl<Context, Input> PositiveWitness<Context, Input> {
    pub fn new(name: impl Into<String>, context: Context, input: Input) -> Self {
        Self {
            name: name.into(),
            context,
            input,
            canonical: false,
        }
    }

    pub fn with_canonical(mut self, canonical: bool) -> Self {
        self.canonical = canonical;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn input(&self) -> &Input {
        &self.input
    }

    pub fn canonical(&self) -> bool {
        self.canonical
    }
}

/// Concrete input that should not follow an abstract transition.
#[derive(Debug, Clone)]
pub struct NegativeWitness<Context, Input> {
    name: String,
    context: Context,
    input: Input,
}

impl<Context, Input> NegativeWitness<Context, Input> {
    pub fn new(name: impl Into<String>, context: Context, input: Input) -> Self {
        Self {
            name: name.into(),
            context,
            input,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn input(&self) -> &Input {
        &self.input
    }
}

/// Declares how an abstract action is encoded as a concrete witness input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessKind {
    CanonicalPositive,
    Positive,
    Negative,
}

pub trait ProtocolInputWitnessCodec<Action>: Clone + Debug {
    fn canonical_positive(action: &Action) -> Self;

    fn positive_family(action: &Action) -> Vec<Self> {
        vec![Self::canonical_positive(action)]
    }

    fn negative_family(action: &Action) -> Vec<Self> {
        vec![Self::canonical_positive(action)]
    }

    fn witness_name(_action: &Action, kind: WitnessKind, index: usize) -> String {
        match kind {
            WitnessKind::CanonicalPositive => "principal".to_owned(),
            WitnessKind::Positive => format!("positive_{index}"),
            WitnessKind::Negative => format!("negative_{index}"),
        }
    }
}

pub enum WitnessFamily<'a, Context, Input> {
    Positive(&'a [PositiveWitness<Context, Input>]),
    Negative(&'a [NegativeWitness<Context, Input>]),
}

/// Binding that can materialize concrete runtime inputs for abstract conformance actions.
#[allow(async_fn_in_trait)]
pub trait ProtocolInputWitnessBinding<Spec>: ProtocolRuntimeBinding<Spec>
where
    Spec: ProtocolConformanceSpec,
{
    type Input: Clone + Debug;
    type Session;

    async fn fresh_session(spec: &Spec) -> Self::Session;

    fn positive_witnesses(
        spec: &Spec,
        session: &Self::Session,
        prev: &Spec::State,
        action: &Spec::Action,
        next: &Spec::State,
    ) -> Vec<PositiveWitness<Self::Context, Self::Input>>;

    fn negative_witnesses(
        spec: &Spec,
        session: &Self::Session,
        prev: &Spec::State,
        action: &Spec::Action,
    ) -> Vec<NegativeWitness<Self::Context, Self::Input>>;

    async fn execute_input(
        runtime: &Self::Runtime,
        session: &mut Self::Session,
        context: &Self::Context,
        input: &Self::Input,
    ) -> Spec::ProbeOutput;

    fn probe_context(session: &Self::Session) -> Self::Context;
}

/// Dynamically built test case used by the witness harness.
pub struct DynamicTestCase {
    name: String,
    run: Box<dyn Fn() -> Result<(), String>>,
}

impl DynamicTestCase {
    pub fn new<F>(name: impl Into<String>, run: F) -> Self
    where
        F: Fn() -> Result<(), String> + 'static,
    {
        Self {
            name: name.into(),
            run: Box::new(run),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn execute(&self) -> Result<(), String> {
        (self.run)()
    }
}

/// Inventory entry that contributes witness tests to the custom harness.
pub struct RegisteredCodeWitnessTestProvider {
    pub build: fn() -> Vec<DynamicTestCase>,
}

nirvash::inventory::collect!(RegisteredCodeWitnessTestProvider);

pub fn summarize_state<Spec>(spec: &Spec, probe: &Spec::ProbeState) -> Spec::SummaryState
where
    Spec: ProtocolConformanceSpec,
{
    spec.summarize_state(probe)
}

pub fn summarize_output<Spec>(spec: &Spec, probe: &Spec::ProbeOutput) -> Spec::SummaryOutput
where
    Spec: ProtocolConformanceSpec,
{
    spec.summarize_output(probe)
}

pub fn abstract_initial_state<Spec>(spec: &Spec, probe: &Spec::ProbeState) -> Spec::State
where
    Spec: ProtocolConformanceSpec,
{
    let summary = spec.summarize_state(probe);
    spec.abstract_state(&summary)
}

pub fn abstract_next_state<Spec>(spec: &Spec, probe: &Spec::ProbeState) -> Spec::State
where
    Spec: ProtocolConformanceSpec,
{
    let summary = spec.summarize_state(probe);
    spec.abstract_state(&summary)
}

pub fn abstract_expected_output<Spec>(
    spec: &Spec,
    probe: &Spec::ProbeOutput,
) -> Spec::ExpectedOutput
where
    Spec: ProtocolConformanceSpec,
{
    let summary = spec.summarize_output(probe);
    spec.abstract_output(&summary)
}

pub fn enabled_from_summary<Spec>(
    spec: &Spec,
    summary: &Spec::SummaryState,
    action: &Spec::Action,
) -> bool
where
    Spec: ProtocolConformanceSpec,
{
    let projected = spec.abstract_state(summary);
    !<Spec as FrontendSpec>::transition_relation(spec, &projected, action).is_empty()
}

struct SummaryRefinementMap<'a, Spec>(&'a Spec);

impl<Spec> RefinementMap<Spec> for SummaryRefinementMap<'_, Spec>
where
    Spec: ProtocolConformanceSpec,
{
    type ImplState = Spec::SummaryState;
    type ImplInput = Spec::Action;
    type ImplOutput = ();
    type AuxState = ();

    fn abstract_state(&self, state: &Self::ImplState, _aux: &Self::AuxState) -> Spec::State {
        self.0.abstract_state(state)
    }

    fn candidate_actions(
        &self,
        _before: &Self::ImplState,
        input: &Self::ImplInput,
        _output: &Self::ImplOutput,
        _after: &Self::ImplState,
        _aux: &Self::AuxState,
    ) -> Vec<Spec::Action> {
        vec![input.clone()]
    }
}

impl<Spec> TraceRefinementMap<Spec> for SummaryRefinementMap<'_, Spec>
where
    Spec: ProtocolConformanceSpec,
{
    type ImplState = Spec::SummaryState;
    type ImplInput = ();
    type ImplOutput = Spec::SummaryOutput;
    type AuxState = ();

    fn init_aux(&self, _initial: &Self::ImplState) -> Self::AuxState {}

    fn next_aux(
        &self,
        _before: &Self::ImplState,
        _event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        _after: &Self::ImplState,
        _aux: &Self::AuxState,
    ) -> Self::AuxState {
    }

    fn abstract_state(&self, state: &Self::ImplState, _aux: &Self::AuxState) -> Spec::State {
        self.0.abstract_state(state)
    }

    fn candidate_actions(
        &self,
        _before: &Self::ImplState,
        event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        _after: &Self::ImplState,
        _aux: &Self::AuxState,
    ) -> Vec<Spec::Action> {
        match event {
            ObservedEvent::Action { action, .. } => vec![action.clone()],
            _ => Vec::new(),
        }
    }

    fn output_matches(
        &self,
        spec: &Spec,
        action: &Spec::Action,
        abstract_before: &Spec::State,
        abstract_after: &Spec::State,
        event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        _aux: &Self::AuxState,
    ) -> bool {
        match event {
            ObservedEvent::Action { output, .. } => {
                let expected_output =
                    spec.expected_output(abstract_before, action, Some(abstract_after));
                spec.abstract_output(output) == expected_output
            }
            ObservedEvent::Stutter | ObservedEvent::Internal => abstract_before == abstract_after,
            ObservedEvent::Invoke { .. } | ObservedEvent::Return { .. } => false,
        }
    }

    fn hidden_step(
        &self,
        _before: &Self::ImplState,
        event: &ObservedEvent<Spec::Action, Self::ImplOutput, Self::ImplInput>,
        _after: &Self::ImplState,
        _aux: &Self::AuxState,
    ) -> bool {
        matches!(event, ObservedEvent::Internal | ObservedEvent::Stutter)
    }
}

pub fn step_refines_relation<Spec, R>(
    spec: &Spec,
    map: &R,
    before: &R::ImplState,
    input: &R::ImplInput,
    output: &R::ImplOutput,
    after: &R::ImplState,
    aux: &R::AuxState,
) -> Result<
    StepRefinementWitness<Spec::State, Spec::Action>,
    StepRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: FrontendSpec,
    R: RefinementMap<Spec>,
{
    let abstract_before = map.abstract_state(before, aux);
    let abstract_after = map.abstract_state(after, aux);
    let candidate_actions = map.candidate_actions(before, input, output, after, aux);

    if candidate_actions.is_empty() {
        return Err(StepRefinementError::NoCandidateActions {
            abstract_before,
            abstract_after,
        });
    }

    for action in &candidate_actions {
        if <Spec as FrontendSpec>::contains_transition(
            spec,
            &abstract_before,
            action,
            &abstract_after,
        ) {
            return Ok(StepRefinementWitness {
                abstract_before,
                chosen_action: action.clone(),
                abstract_after,
            });
        }
    }

    Err(StepRefinementError::NoMatchingAbstractSuccessor {
        abstract_before,
        abstract_after,
        candidate_actions,
    })
}

pub fn step_refines_summary<Spec>(
    spec: &Spec,
    before_summary: &Spec::SummaryState,
    action: &Spec::Action,
    after_summary: &Spec::SummaryState,
) -> Result<
    StepRefinementWitness<Spec::State, Spec::Action>,
    StepRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec,
{
    let map = SummaryRefinementMap(spec);
    step_refines_relation(spec, &map, before_summary, action, &(), after_summary, &())
}

pub fn assert_initial_refinement<Spec>(spec: &Spec, summary: &Spec::SummaryState)
where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
{
    let projected = spec.abstract_state(summary);
    let initial_states = <Spec as FrontendSpec>::initial_states(spec);
    assert!(
        initial_states.contains(&projected),
        "runtime initial state {:?} must be one of the declared initial states {:?}",
        projected,
        initial_states,
    );
}

pub fn assert_output_refinement<Spec>(
    spec: &Spec,
    before_summary: &Spec::SummaryState,
    action: &Spec::Action,
    after_summary: &Spec::SummaryState,
    output_summary: &Spec::SummaryOutput,
) where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
{
    let before = spec.abstract_state(before_summary);
    let next = spec.abstract_state(after_summary);
    let expected_output = spec.expected_output(&before, action, Some(&next));
    let projected_output = spec.abstract_output(output_summary);
    assert_eq!(
        projected_output, expected_output,
        "summary/state output mismatch for {action:?} from {before_summary:?}",
    );
}

fn trace_refines_summary_with_label<Spec>(
    spec: &Spec,
    observed: &ObservedTrace<Spec::SummaryState, Spec::Action, Spec::SummaryOutput>,
    abstract_trace: &Trace<Spec::State, Spec::Action>,
    model_case_label: String,
) -> Result<
    TraceRefinementWitness<Spec::State, Spec::Action>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
{
    let candidate_trace = abstract_trace.clone();
    let candidate_trace_option =
        |candidate_trace: &Trace<Spec::State, Spec::Action>| Some(candidate_trace.clone());
    let shape_error = |detail: String| TraceRefinementError::ShapeMismatch {
        model_case_label: model_case_label.clone(),
        detail,
        candidate_trace: candidate_trace_option(&candidate_trace),
    };

    if observed.is_empty() {
        return Err(shape_error("observed trace has no states".to_owned()));
    }
    if !observed.is_total_observation() {
        return Err(shape_error(
            "explicit candidate matching requires total state observation".to_owned(),
        ));
    }
    if observed.events().len() != observed.states().len() {
        return Err(shape_error(format!(
            "observed trace has {} states but {} events",
            observed.states().len(),
            observed.events().len()
        )));
    }
    let Some(observed_loop_start) = observed.loop_start() else {
        return Err(shape_error(
            "explicit candidate matching requires a lasso loop_start".to_owned(),
        ));
    };
    if observed_loop_start >= observed.states().len() {
        return Err(shape_error(format!(
            "observed loop_start {} is outside state length {}",
            observed_loop_start,
            observed.states().len()
        )));
    }
    if observed.states().len() != abstract_trace.states().len() {
        return Err(shape_error(format!(
            "observed trace has {} states but candidate has {}",
            observed.states().len(),
            abstract_trace.states().len()
        )));
    }
    if observed.events().len() != abstract_trace.steps().len() {
        return Err(shape_error(format!(
            "observed trace has {} events but candidate has {} steps",
            observed.events().len(),
            abstract_trace.steps().len()
        )));
    }
    if observed_loop_start != abstract_trace.loop_start() {
        return Err(shape_error(format!(
            "observed loop_start {} does not match candidate loop_start {}",
            observed_loop_start,
            abstract_trace.loop_start()
        )));
    }

    let observed_initial = spec.abstract_state(
        observed
            .observed_state(0)
            .expect("total observation guarantees an initial state"),
    );
    let abstract_initial = abstract_trace.states()[0].clone();
    if observed_initial != abstract_initial {
        return Err(TraceRefinementError::InitialStateMismatch {
            model_case_label,
            observed_initial,
            abstract_initial,
            candidate_trace: candidate_trace_option(&candidate_trace),
        });
    }

    let mut step_witnesses = Vec::with_capacity(observed.events().len());
    for index in 0..observed.events().len() {
        let step = abstract_trace.steps()[index].clone();
        let abstract_before = abstract_trace.states()[index].clone();
        let expected_after = abstract_trace.states()[abstract_trace.next_index(index)].clone();
        let observed_before = spec.abstract_state(
            observed
                .observed_state(index)
                .expect("total observation guarantees a state at each index"),
        );
        let observed_after_summary = observed
            .observed_state(observed.next_index(index))
            .expect("total observation guarantees a successor state");
        let observed_after = spec.abstract_state(observed_after_summary);

        if observed_before != abstract_before {
            return Err(TraceRefinementError::StepMismatch {
                model_case_label: model_case_label.clone(),
                index,
                matching_prefix_len: index,
                step,
                abstract_before,
                abstract_after: observed_before,
                expected_after,
                detail: "observed abstract state does not match candidate prefix state".to_owned(),
                refinement_error: None,
                candidate_trace: candidate_trace_option(&candidate_trace),
            });
        }

        match (&abstract_trace.steps()[index], &observed.events()[index]) {
            (TraceStep::Action(expected_action), ObservedEvent::Action { action, output }) => {
                if action != expected_action {
                    return Err(TraceRefinementError::StepMismatch {
                        model_case_label: model_case_label.clone(),
                        index,
                        matching_prefix_len: index,
                        step: TraceStep::Action(expected_action.clone()),
                        abstract_before,
                        abstract_after: observed_after,
                        expected_after,
                        detail: "observed action does not match candidate action".to_owned(),
                        refinement_error: None,
                        candidate_trace: candidate_trace_option(&candidate_trace),
                    });
                }

                let refinement = step_refines_summary(
                    spec,
                    observed
                        .observed_state(index)
                        .expect("total observation guarantees a state at each index"),
                    action,
                    observed_after_summary,
                )
                .map_err(|error| TraceRefinementError::StepMismatch {
                    model_case_label: model_case_label.clone(),
                    index,
                    matching_prefix_len: index,
                    step: TraceStep::Action(expected_action.clone()),
                    abstract_before: abstract_before.clone(),
                    abstract_after: observed_after.clone(),
                    expected_after: expected_after.clone(),
                    detail: "summary step does not refine candidate transition".to_owned(),
                    refinement_error: Some(error),
                    candidate_trace: candidate_trace_option(&candidate_trace),
                })?;

                if refinement.abstract_after != expected_after {
                    return Err(TraceRefinementError::StepMismatch {
                        model_case_label: model_case_label.clone(),
                        index,
                        matching_prefix_len: index,
                        step: TraceStep::Action(expected_action.clone()),
                        abstract_before,
                        abstract_after: refinement.abstract_after,
                        expected_after,
                        detail: "observed abstract successor does not match candidate successor"
                            .to_owned(),
                        refinement_error: None,
                        candidate_trace: candidate_trace_option(&candidate_trace),
                    });
                }

                let expected_output = spec.expected_output(
                    &refinement.abstract_before,
                    expected_action,
                    Some(&expected_after),
                );
                let observed_output = spec.abstract_output(output);
                if observed_output != expected_output {
                    return Err(TraceRefinementError::StepMismatch {
                        model_case_label: model_case_label.clone(),
                        index,
                        matching_prefix_len: index,
                        step: TraceStep::Action(expected_action.clone()),
                        abstract_before: refinement.abstract_before,
                        abstract_after: refinement.abstract_after,
                        expected_after,
                        detail: format!(
                            "expected output {:?} but observed {:?}",
                            expected_output, observed_output
                        ),
                        refinement_error: None,
                        candidate_trace: candidate_trace_option(&candidate_trace),
                    });
                }

                step_witnesses.push(TraceStepRefinementWitness {
                    index,
                    abstract_before: refinement.abstract_before,
                    step: TraceStep::Action(expected_action.clone()),
                    abstract_after: refinement.abstract_after,
                });
            }
            (TraceStep::Action(expected_action), ObservedEvent::Stutter) => {
                return Err(TraceRefinementError::StepMismatch {
                    model_case_label: model_case_label.clone(),
                    index,
                    matching_prefix_len: index,
                    step: TraceStep::Action(expected_action.clone()),
                    abstract_before,
                    abstract_after: observed_after,
                    expected_after,
                    detail: "observed stutter does not match candidate action".to_owned(),
                    refinement_error: None,
                    candidate_trace: candidate_trace_option(&candidate_trace),
                });
            }
            (TraceStep::Stutter, ObservedEvent::Action { action: _, .. }) => {
                return Err(TraceRefinementError::StutterMismatch {
                    model_case_label: model_case_label.clone(),
                    index,
                    matching_prefix_len: index,
                    abstract_before,
                    abstract_after: observed_after,
                    detail: "observed action does not match candidate stutter".to_owned(),
                    candidate_trace: candidate_trace_option(&candidate_trace),
                });
            }
            (TraceStep::Stutter, ObservedEvent::Stutter) => {
                if abstract_before != expected_after {
                    return Err(TraceRefinementError::StutterMismatch {
                        model_case_label: model_case_label.clone(),
                        index,
                        matching_prefix_len: index,
                        abstract_before,
                        abstract_after: expected_after,
                        detail: "candidate stutter must stay in the same abstract state".to_owned(),
                        candidate_trace: candidate_trace_option(&candidate_trace),
                    });
                }
                if observed_before != observed_after {
                    return Err(TraceRefinementError::StutterMismatch {
                        model_case_label: model_case_label.clone(),
                        index,
                        matching_prefix_len: index,
                        abstract_before,
                        abstract_after: observed_after,
                        detail: "observed stutter changed the abstract state".to_owned(),
                        candidate_trace: candidate_trace_option(&candidate_trace),
                    });
                }
                if observed_after != expected_after {
                    return Err(TraceRefinementError::StutterMismatch {
                        model_case_label: model_case_label.clone(),
                        index,
                        matching_prefix_len: index,
                        abstract_before,
                        abstract_after: observed_after,
                        detail: "observed stutter state does not match candidate state".to_owned(),
                        candidate_trace: candidate_trace_option(&candidate_trace),
                    });
                }

                step_witnesses.push(TraceStepRefinementWitness {
                    index,
                    abstract_before,
                    step: TraceStep::Stutter,
                    abstract_after: expected_after,
                });
            }
            (TraceStep::Action(expected_action), event) => {
                return Err(TraceRefinementError::StepMismatch {
                    model_case_label: model_case_label.clone(),
                    index,
                    matching_prefix_len: index,
                    step: TraceStep::Action(expected_action.clone()),
                    abstract_before,
                    abstract_after: observed_after,
                    expected_after,
                    detail: format!("observed event {:?} does not match candidate action", event),
                    refinement_error: None,
                    candidate_trace: candidate_trace_option(&candidate_trace),
                });
            }
            (TraceStep::Stutter, event) => {
                return Err(TraceRefinementError::StutterMismatch {
                    model_case_label: model_case_label.clone(),
                    index,
                    matching_prefix_len: index,
                    abstract_before,
                    abstract_after: observed_after,
                    detail: format!(
                        "observed event {:?} does not match candidate stutter",
                        event
                    ),
                    candidate_trace: candidate_trace_option(&candidate_trace),
                });
            }
        }
    }

    Ok(TraceRefinementWitness {
        abstract_trace: candidate_trace,
        steps: step_witnesses,
        model_case_label,
    })
}

pub fn trace_refines_summary<Spec>(
    spec: &Spec,
    observed: &ObservedTrace<Spec::SummaryState, Spec::Action, Spec::SummaryOutput>,
    abstract_trace: &Trace<Spec::State, Spec::Action>,
) -> Result<
    TraceRefinementWitness<Spec::State, Spec::Action>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
{
    trace_refines_summary_with_label(spec, observed, abstract_trace, "direct".to_owned())
}

fn observed_matches_candidate_trace<SummaryState, A, SummaryOutput, SummaryInput, S>(
    observed: &ObservedTrace<SummaryState, A, SummaryOutput, SummaryInput>,
    candidate: &Trace<S, A>,
) -> bool
where
    A: PartialEq,
{
    observed.loop_start() == Some(candidate.loop_start())
        && observed.events().len() == candidate.steps().len()
        && observed
            .events()
            .iter()
            .zip(candidate.steps())
            .all(|(event, step)| event.matches_step(step))
}

fn prefer_trace_refinement_error<S, A>(
    current: Option<&TraceRefinementError<S, A>>,
    next: &TraceRefinementError<S, A>,
) -> bool {
    let Some(current) = current else {
        return true;
    };
    next.matching_prefix_len() > current.matching_prefix_len()
}

fn observed_uses_lasso<SummaryState, A, SummaryOutput, SummaryInput>(
    observed: &ObservedTrace<SummaryState, A, SummaryOutput, SummaryInput>,
) -> bool {
    observed
        .loop_start()
        .is_some_and(|loop_start| loop_start + 1 != observed.states().len())
}

fn observed_state_matches_candidate<Spec, Map>(
    map: &Map,
    observation: &StateObservation<Map::ImplState>,
    candidate_state: &Spec::State,
    aux: &Map::AuxState,
) -> bool
where
    Spec: FrontendSpec,
    Spec::State: PartialEq,
    Map: TraceRefinementMap<Spec>,
{
    observation
        .as_ref()
        .map(|state| map.abstract_state(state, aux) == *candidate_state)
        .unwrap_or(true)
}

fn observed_hidden_step<Spec, Map>(
    map: &Map,
    before: Option<&Map::ImplState>,
    event: &ObservedEvent<Spec::Action, Map::ImplOutput, Map::ImplInput>,
    after: Option<&Map::ImplState>,
    aux: &Map::AuxState,
) -> bool
where
    Spec: FrontendSpec,
    Map: TraceRefinementMap<Spec>,
{
    match (before, after) {
        (Some(before), Some(after)) => map.hidden_step(before, event, after, aux),
        _ => matches!(event, ObservedEvent::Internal | ObservedEvent::Stutter),
    }
}

fn observed_next_aux<Spec, Map>(
    map: &Map,
    before: Option<&Map::ImplState>,
    event: &ObservedEvent<Spec::Action, Map::ImplOutput, Map::ImplInput>,
    after: Option<&Map::ImplState>,
    aux: &Map::AuxState,
) -> Map::AuxState
where
    Spec: FrontendSpec,
    Map: TraceRefinementMap<Spec>,
{
    match (before, after) {
        (Some(before), Some(after)) => map.next_aux(before, event, after, aux),
        _ => aux.clone(),
    }
}

fn observed_candidate_actions<Spec, Map>(
    spec: &Spec,
    map: &Map,
    before: Option<&Map::ImplState>,
    event: &ObservedEvent<Spec::Action, Map::ImplOutput, Map::ImplInput>,
    after: Option<&Map::ImplState>,
    aux: &Map::AuxState,
) -> Vec<Spec::Action>
where
    Spec: FrontendSpec,
    Spec::Action: Clone,
    Map: TraceRefinementMap<Spec>,
{
    match (before, after) {
        (Some(before), Some(after)) => map.candidate_actions(before, event, after, aux),
        _ => match event {
            ObservedEvent::Action { action, .. } => vec![action.clone()],
            ObservedEvent::Invoke { .. } | ObservedEvent::Return { .. } => spec.actions(),
            ObservedEvent::Internal | ObservedEvent::Stutter => Vec::new(),
        },
    }
}

fn terminal_candidate_trace<S, A>(states: Vec<S>, mut steps: Vec<TraceStep<A>>) -> Trace<S, A> {
    let loop_start = states.len().saturating_sub(1);
    steps.push(TraceStep::Stutter);
    Trace::new(states, steps, loop_start)
}

fn collect_symbolic_prefix_traces<S, A>(
    graph: &ReachableGraphSnapshot<S, A>,
    state_index: usize,
    max_depth: usize,
    allow_lasso: bool,
    path_indices: &mut Vec<usize>,
    path_states: &mut Vec<S>,
    path_steps: &mut Vec<TraceStep<A>>,
    traces: &mut Vec<Trace<S, A>>,
) where
    S: Clone + PartialEq,
    A: Clone + PartialEq,
{
    traces.push(terminal_candidate_trace(
        path_states.clone(),
        path_steps.clone(),
    ));
    if path_steps.len() >= max_depth {
        return;
    }

    for edge in &graph.edges[state_index] {
        if let Some(loop_start) = path_indices
            .iter()
            .position(|existing| *existing == edge.target)
        {
            if allow_lasso {
                let mut lasso_steps = path_steps.clone();
                lasso_steps.push(TraceStep::Action(edge.action.clone()));
                traces.push(Trace::new(path_states.clone(), lasso_steps, loop_start));
            }
            continue;
        }

        path_indices.push(edge.target);
        path_states.push(graph.states[edge.target].clone());
        path_steps.push(TraceStep::Action(edge.action.clone()));
        collect_symbolic_prefix_traces(
            graph,
            edge.target,
            max_depth,
            allow_lasso,
            path_indices,
            path_states,
            path_steps,
            traces,
        );
        path_steps.pop();
        path_states.pop();
        path_indices.pop();
    }
}

fn match_candidate_trace_from<Spec, Map>(
    spec: &Spec,
    map: &Map,
    observed: &ObservedTrace<Map::ImplState, Spec::Action, Map::ImplOutput, Map::ImplInput>,
    candidate: &Trace<Spec::State, Spec::Action>,
    model_case_label: &str,
    observed_index: usize,
    consumed_steps: usize,
    candidate_index: usize,
    aux: Map::AuxState,
    hidden_steps_since_match: usize,
    config: &TraceRefinementConfig,
    step_witnesses: Vec<TraceStepRefinementWitness<Spec::State, Spec::Action>>,
) -> Result<
    Vec<TraceStepRefinementWitness<Spec::State, Spec::Action>>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
    Spec::Action: Clone + PartialEq,
    Map: TraceRefinementMap<Spec>,
{
    let candidate_trace_option = || Some(candidate.clone());
    if observed_index == observed.events().len() {
        return if consumed_steps == candidate.steps().len() {
            Ok(step_witnesses)
        } else {
            Err(TraceRefinementError::ShapeMismatch {
                model_case_label: model_case_label.to_owned(),
                detail: "candidate has remaining abstract steps after observations ended"
                    .to_owned(),
                candidate_trace: candidate_trace_option(),
            })
        };
    }

    let event = &observed.events()[observed_index];
    let before_observation = &observed.states()[observed_index];
    let after_index = observed.next_index(observed_index);
    let after_observation = &observed.states()[after_index];
    let before_impl = before_observation.as_ref();
    let after_impl = after_observation.as_ref();
    let next_aux = observed_next_aux::<Spec, Map>(map, before_impl, event, after_impl, &aux);

    let mut best_error = None;

    if observed_hidden_step::<Spec, Map>(map, before_impl, event, after_impl, &aux)
        && hidden_steps_since_match < config.max_hidden_steps_between_observations
    {
        let candidate_state = &candidate.states()[candidate_index];
        if observed_state_matches_candidate::<Spec, Map>(
            map,
            before_observation,
            candidate_state,
            &aux,
        ) && observed_state_matches_candidate::<Spec, Map>(
            map,
            after_observation,
            candidate_state,
            &next_aux,
        ) {
            match match_candidate_trace_from(
                spec,
                map,
                observed,
                candidate,
                model_case_label,
                observed_index + 1,
                consumed_steps,
                candidate_index,
                next_aux.clone(),
                hidden_steps_since_match + 1,
                config,
                step_witnesses.clone(),
            ) {
                Ok(steps) => return Ok(steps),
                Err(error) => {
                    if prefer_trace_refinement_error(best_error.as_ref(), &error) {
                        best_error = Some(error);
                    }
                }
            }
        }
    }

    if consumed_steps >= candidate.steps().len() {
        return Err(
            best_error.unwrap_or_else(|| TraceRefinementError::ShapeMismatch {
                model_case_label: model_case_label.to_owned(),
                detail: "observed trace has additional visible events after candidate exhaustion"
                    .to_owned(),
                candidate_trace: candidate_trace_option(),
            }),
        );
    }

    let candidate_before = candidate.states()[candidate_index].clone();
    let next_candidate_index = candidate.next_index(candidate_index);
    let candidate_after = candidate.states()[next_candidate_index].clone();

    if !observed_state_matches_candidate::<Spec, Map>(
        map,
        before_observation,
        &candidate_before,
        &aux,
    ) {
        return Err(TraceRefinementError::StepMismatch {
            model_case_label: model_case_label.to_owned(),
            index: observed_index,
            matching_prefix_len: consumed_steps,
            step: candidate.steps()[candidate_index].clone(),
            abstract_before: candidate_before.clone(),
            abstract_after: candidate_before.clone(),
            expected_after: candidate_after.clone(),
            detail: "observed state does not match candidate prefix state".to_owned(),
            refinement_error: None,
            candidate_trace: candidate_trace_option(),
        });
    }

    if !observed_state_matches_candidate::<Spec, Map>(
        map,
        after_observation,
        &candidate_after,
        &next_aux,
    ) {
        return Err(TraceRefinementError::StepMismatch {
            model_case_label: model_case_label.to_owned(),
            index: observed_index,
            matching_prefix_len: consumed_steps,
            step: candidate.steps()[candidate_index].clone(),
            abstract_before: candidate_before.clone(),
            abstract_after: candidate_after.clone(),
            expected_after: candidate_after.clone(),
            detail: "observed successor does not match candidate successor".to_owned(),
            refinement_error: None,
            candidate_trace: candidate_trace_option(),
        });
    }

    match &candidate.steps()[candidate_index] {
        TraceStep::Action(action) => {
            let candidate_actions = observed_candidate_actions::<Spec, Map>(
                spec,
                map,
                before_impl,
                event,
                after_impl,
                &aux,
            );
            if !candidate_actions
                .iter()
                .any(|candidate_action| candidate_action == action)
            {
                return Err(TraceRefinementError::StepMismatch {
                    model_case_label: model_case_label.to_owned(),
                    index: observed_index,
                    matching_prefix_len: consumed_steps,
                    step: TraceStep::Action(action.clone()),
                    abstract_before: candidate_before,
                    abstract_after: candidate_after.clone(),
                    expected_after: candidate_after,
                    detail: format!(
                        "candidate action {:?} is not allowed by observed event",
                        action
                    ),
                    refinement_error: None,
                    candidate_trace: candidate_trace_option(),
                });
            }
            if !map.output_matches(
                spec,
                action,
                &candidate.states()[candidate_index],
                &candidate.states()[next_candidate_index],
                event,
                &aux,
            ) {
                return Err(TraceRefinementError::StepMismatch {
                    model_case_label: model_case_label.to_owned(),
                    index: observed_index,
                    matching_prefix_len: consumed_steps,
                    step: TraceStep::Action(action.clone()),
                    abstract_before: candidate_before,
                    abstract_after: candidate_after.clone(),
                    expected_after: candidate_after,
                    detail: "observed output does not match candidate action".to_owned(),
                    refinement_error: None,
                    candidate_trace: candidate_trace_option(),
                });
            }

            let mut next_steps = step_witnesses;
            next_steps.push(TraceStepRefinementWitness {
                index: observed_index,
                abstract_before: candidate.states()[candidate_index].clone(),
                step: TraceStep::Action(action.clone()),
                abstract_after: candidate.states()[next_candidate_index].clone(),
            });
            match_candidate_trace_from(
                spec,
                map,
                observed,
                candidate,
                model_case_label,
                observed_index + 1,
                consumed_steps + 1,
                next_candidate_index,
                next_aux,
                0,
                config,
                next_steps,
            )
        }
        TraceStep::Stutter => {
            if !matches!(event, ObservedEvent::Stutter) {
                return Err(TraceRefinementError::StutterMismatch {
                    model_case_label: model_case_label.to_owned(),
                    index: observed_index,
                    matching_prefix_len: consumed_steps,
                    abstract_before: candidate_before,
                    abstract_after: candidate_after,
                    detail: "non-stutter event cannot consume candidate stutter".to_owned(),
                    candidate_trace: candidate_trace_option(),
                });
            }
            let mut next_steps = step_witnesses;
            next_steps.push(TraceStepRefinementWitness {
                index: observed_index,
                abstract_before: candidate.states()[candidate_index].clone(),
                step: TraceStep::Stutter,
                abstract_after: candidate.states()[next_candidate_index].clone(),
            });
            match_candidate_trace_from(
                spec,
                map,
                observed,
                candidate,
                model_case_label,
                observed_index + 1,
                consumed_steps + 1,
                next_candidate_index,
                next_aux,
                0,
                config,
                next_steps,
            )
        }
    }
}

fn match_candidate_trace<Spec, Map>(
    spec: &Spec,
    map: &Map,
    observed: &ObservedTrace<Map::ImplState, Spec::Action, Map::ImplOutput, Map::ImplInput>,
    candidate: &Trace<Spec::State, Spec::Action>,
    model_case_label: &str,
    config: &TraceRefinementConfig,
) -> Result<
    TraceRefinementWitness<Spec::State, Spec::Action>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
    Spec::Action: Clone + PartialEq,
    Map: TraceRefinementMap<Spec>,
{
    let candidate_trace_option = || Some(candidate.clone());
    let Some(initial) = observed.states().first().and_then(StateObservation::as_ref) else {
        return Err(TraceRefinementError::ShapeMismatch {
            model_case_label: model_case_label.to_owned(),
            detail: "observed trace requires an initial state observation".to_owned(),
            candidate_trace: None,
        });
    };
    let aux = map.init_aux(initial);
    let observed_initial = map.abstract_state(initial, &aux);
    let abstract_initial = candidate.states()[0].clone();
    if observed_initial != abstract_initial {
        return Err(TraceRefinementError::InitialStateMismatch {
            model_case_label: model_case_label.to_owned(),
            observed_initial,
            abstract_initial,
            candidate_trace: candidate_trace_option(),
        });
    }

    let steps = match_candidate_trace_from(
        spec,
        map,
        observed,
        candidate,
        model_case_label,
        0,
        0,
        0,
        aux,
        0,
        config,
        Vec::new(),
    )?;
    Ok(TraceRefinementWitness {
        abstract_trace: candidate.clone(),
        steps,
        model_case_label: model_case_label.to_owned(),
    })
}

pub fn constrained_trace_refines<Spec, Map>(
    spec: &Spec,
    map: &Map,
    model_case: ModelInstance<Spec::State, Spec::Action>,
    observed: &ObservedTrace<Map::ImplState, Spec::Action, Map::ImplOutput, Map::ImplInput>,
    config: TraceRefinementConfig,
) -> Result<
    TraceRefinementWitness<Spec::State, Spec::Action>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec + TemporalSpec,
    Spec::State: Clone + PartialEq + FiniteModelDomain + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Send + Sync + 'static,
    Map: TraceRefinementMap<Spec>,
{
    let model_case_label = model_case.label().to_owned();
    if observed.is_empty() {
        return Err(TraceRefinementError::ShapeMismatch {
            model_case_label,
            detail: "observed trace has no states".to_owned(),
            candidate_trace: None,
        });
    }
    if observed.events().len() != observed.states().len() {
        return Err(TraceRefinementError::ShapeMismatch {
            model_case_label,
            detail: format!(
                "observed trace has {} states but {} events",
                observed.states().len(),
                observed.events().len()
            ),
            candidate_trace: None,
        });
    }
    if observed
        .loop_start()
        .is_some_and(|loop_start| loop_start >= observed.states().len())
    {
        return Err(TraceRefinementError::ShapeMismatch {
            model_case_label,
            detail: "observed loop_start is outside the state sequence".to_owned(),
            candidate_trace: None,
        });
    }
    if config.require_total_observation && !observed.is_total_observation() {
        return Err(TraceRefinementError::ShapeMismatch {
            model_case_label,
            detail: "selected refinement engine requires total state observation".to_owned(),
            candidate_trace: None,
        });
    }
    if !config.allow_lasso && observed_uses_lasso(observed) {
        return Err(TraceRefinementError::ShapeMismatch {
            model_case_label,
            detail: "selected refinement engine does not allow lasso observations".to_owned(),
            candidate_trace: None,
        });
    }

    let mut lowering_cx = LoweringCx;
    let lowered =
        spec.lower(&mut lowering_cx)
            .map_err(|error| TraceRefinementError::SearchFailed {
                model_case_label: model_case.label().to_owned(),
                detail: error.to_string(),
            })?;
    let max_depth = observed.events().len();

    let candidates = match config.engine {
        TraceRefinementEngine::ExplicitCandidate | TraceRefinementEngine::ExplicitConstrained => {
            let mut checker_config = ModelCheckConfig::bounded_lasso(max_depth);
            checker_config.backend = Some(ModelBackend::Explicit);
            let candidate_case = model_case.clone().with_checker_config(checker_config);
            let mut traces =
                nirvash_check::ExplicitModelChecker::for_case(&lowered, candidate_case)
                    .candidate_traces()
                    .map_err(|error| TraceRefinementError::SearchFailed {
                        model_case_label: model_case.label().to_owned(),
                        detail: format!("{:?}: {error:?}", config.engine),
                    })?;
            if !config.allow_lasso {
                traces.retain(|trace| trace.loop_start() + 1 == trace.states().len());
            }
            traces
        }
        TraceRefinementEngine::SymbolicConstrained => {
            let normalized =
                lowered
                    .normalized_core()
                    .map_err(|error| TraceRefinementError::SearchFailed {
                        model_case_label: model_case.label().to_owned(),
                        detail: format!("symbolic normalization failed: {error}"),
                    })?;
            let unsupported = normalized.fragment_profile().symbolic_unsupported_reasons();
            if !unsupported.is_empty() {
                return Err(TraceRefinementError::ShapeMismatch {
                    model_case_label,
                    detail: format!(
                        "symbolic constrained refinement does not support {}",
                        unsupported.join(", ")
                    ),
                    candidate_trace: None,
                });
            }
            if !lowered.core().fairness.is_empty() || !lowered.core().temporal_props.is_empty() {
                return Err(TraceRefinementError::ShapeMismatch {
                    model_case_label,
                    detail: "symbolic constrained refinement only supports finite-prefix safety fragments".to_owned(),
                    candidate_trace: None,
                });
            }
            if observed_uses_lasso(observed) {
                return Err(TraceRefinementError::ShapeMismatch {
                    model_case_label,
                    detail: "symbolic constrained refinement does not support lasso observations"
                        .to_owned(),
                    candidate_trace: None,
                });
            }

            let mut checker_config = ModelCheckConfig::reachable_graph();
            checker_config.backend = Some(ModelBackend::Symbolic);
            let candidate_case = model_case.clone().with_checker_config(checker_config);
            let mut traces = Vec::new();
            match nirvash_check::SymbolicModelChecker::for_case(&lowered, candidate_case)
                .full_reachable_graph_snapshot()
            {
                Ok(graph) => {
                    for initial in &graph.initial_indices {
                        let mut path_indices = vec![*initial];
                        let mut path_states = vec![graph.states[*initial].clone()];
                        let mut path_steps = Vec::new();
                        collect_symbolic_prefix_traces(
                            &graph,
                            *initial,
                            max_depth.saturating_sub(1),
                            false,
                            &mut path_indices,
                            &mut path_states,
                            &mut path_steps,
                            &mut traces,
                        );
                    }
                }
                Err(nirvash::ModelCheckError::UnsupportedConfiguration(_)) => {
                    let mut fallback_config = ModelCheckConfig::bounded_lasso(max_depth);
                    fallback_config.backend = Some(ModelBackend::Explicit);
                    let fallback_case = model_case.clone().with_checker_config(fallback_config);
                    traces = nirvash_check::ExplicitModelChecker::for_case(&lowered, fallback_case)
                        .candidate_traces()
                        .map_err(|error| TraceRefinementError::SearchFailed {
                            model_case_label: model_case.label().to_owned(),
                            detail: format!(
                                "symbolic constrained fallback search failed: {error:?}"
                            ),
                        })?;
                    traces.retain(|trace| trace.loop_start() + 1 == trace.states().len());
                }
                Err(error) => {
                    return Err(TraceRefinementError::SearchFailed {
                        model_case_label: model_case.label().to_owned(),
                        detail: format!("symbolic constrained search failed: {error:?}"),
                    });
                }
            }
            traces
        }
    };

    let exact_shape = matches!(config.engine, TraceRefinementEngine::ExplicitCandidate)
        && config.max_hidden_steps_between_observations == 0
        && observed.is_total_observation()
        && observed
            .events()
            .iter()
            .all(|event| matches!(event, ObservedEvent::Action { .. } | ObservedEvent::Stutter));

    let mut best_error = None;
    for candidate in candidates {
        if exact_shape && !observed_matches_candidate_trace(observed, &candidate) {
            continue;
        }
        match match_candidate_trace(spec, map, observed, &candidate, model_case.label(), &config) {
            Ok(witness) => return Ok(witness),
            Err(error) => {
                if prefer_trace_refinement_error(best_error.as_ref(), &error) {
                    best_error = Some(error);
                }
            }
        }
    }

    let failure = best_error.unwrap_or_else(|| TraceRefinementError::ShapeMismatch {
        model_case_label: model_case.label().to_owned(),
        detail: format!(
            "no {:?} candidate matched the observed trace",
            config.engine
        ),
        candidate_trace: None,
    });
    Err(TraceRefinementError::NoMatchingExplicitCandidate {
        model_case_label: model_case.label().to_owned(),
        matching_prefix_len: failure.matching_prefix_len(),
        candidate_trace: failure.candidate_trace().cloned(),
        failure: Box::new(failure),
    })
}

pub fn trace_refines<Spec>(
    spec: &Spec,
    model_case: ModelInstance<Spec::State, Spec::Action>,
    observed: &ObservedTrace<Spec::SummaryState, Spec::Action, Spec::SummaryOutput>,
) -> Result<
    TraceRefinementWitness<Spec::State, Spec::Action>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec + TemporalSpec,
    Spec::State: Clone + PartialEq + FiniteModelDomain + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Send + Sync + 'static,
{
    constrained_trace_refines(
        spec,
        &SummaryRefinementMap(spec),
        model_case,
        observed,
        TraceRefinementConfig::default(),
    )
}

pub mod proptest_adapter {
    use super::{
        ObservedTrace, ProtocolConformanceSpec, TraceRefinementError, TraceRefinementWitness,
        trace_refines,
    };
    use nirvash_lower::{ModelInstance, ProofObligation, TemporalSpec};

    pub fn assert_input_sequence_refines<Spec>(
        spec: &Spec,
        model_case: ModelInstance<Spec::State, Spec::Action>,
        observed: &ObservedTrace<Spec::SummaryState, Spec::Action, Spec::SummaryOutput>,
    ) -> Result<
        TraceRefinementWitness<Spec::State, Spec::Action>,
        TraceRefinementError<Spec::State, Spec::Action>,
    >
    where
        Spec: ProtocolConformanceSpec + TemporalSpec,
        Spec::State: PartialEq + nirvash_lower::FiniteModelDomain + Send + Sync,
        Spec::Action: PartialEq + Send + Sync,
    {
        trace_refines(spec, model_case, observed)
    }

    pub fn expected_obligations<S, A>(model_case: &ModelInstance<S, A>) -> Vec<ProofObligation> {
        model_case.reduction_obligations()
    }
}

pub mod loom_adapter {
    use super::{
        ObservedTrace, ProtocolConformanceSpec, TraceRefinementError, TraceRefinementWitness,
        trace_refines,
    };
    use nirvash_lower::{ModelInstance, ProofObligation, TemporalSpec};

    pub fn assert_schedule_refines<Spec>(
        spec: &Spec,
        model_case: ModelInstance<Spec::State, Spec::Action>,
        observed: &ObservedTrace<Spec::SummaryState, Spec::Action, Spec::SummaryOutput>,
    ) -> Result<
        TraceRefinementWitness<Spec::State, Spec::Action>,
        TraceRefinementError<Spec::State, Spec::Action>,
    >
    where
        Spec: ProtocolConformanceSpec + TemporalSpec,
        Spec::State: PartialEq + nirvash_lower::FiniteModelDomain + Send + Sync,
        Spec::Action: PartialEq + Send + Sync,
    {
        trace_refines(spec, model_case, observed)
    }

    pub fn expected_obligations<S, A>(model_case: &ModelInstance<S, A>) -> Vec<ProofObligation> {
        model_case.reduction_obligations()
    }
}

#[cfg(kani)]
pub fn kani_assert_step_refines<Spec>(
    spec: &Spec,
    before_summary: &Spec::SummaryState,
    action: &Spec::Action,
    after_summary: &Spec::SummaryState,
) -> Result<
    StepRefinementWitness<Spec::State, Spec::Action>,
    StepRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: ProtocolConformanceSpec,
{
    step_refines_summary(spec, before_summary, action, after_summary)
}

pub fn assert_declared_state_projection<Summary, State>(
    summary: &Summary,
    expected_summary: &Summary,
    projected_state: &State,
    expected_state: &State,
) where
    Summary: Debug + PartialEq,
    State: Debug + PartialEq,
{
    assert_eq!(
        summary, expected_summary,
        "declared summary projection mismatch",
    );
    assert_eq!(
        projected_state, expected_state,
        "declared abstract state projection mismatch",
    );
}

pub fn assert_declared_output_projection<Output>(
    projected_output: &Output,
    expected_output: &Output,
) where
    Output: Debug + PartialEq,
{
    assert_eq!(
        projected_output, expected_output,
        "declared abstract output projection mismatch",
    );
}

pub fn assert_projection_exhaustive<Input, Output, Domain, Actual, Expected>(
    label: &str,
    domain: Domain,
    actual: Actual,
    expected: Expected,
) where
    Input: Debug,
    Output: Debug + PartialEq,
    Domain: IntoBoundedDomain<Input>,
    Actual: Fn(&Input) -> Output,
    Expected: Fn(&Input) -> Output,
{
    for value in into_bounded_domain(domain).into_vec() {
        let projected = actual(&value);
        let expected_value = expected(&value);
        assert_eq!(
            projected, expected_value,
            "{label}: exhaustive projection mismatch for input {value:?}",
        );
    }
}

fn parse_witness_harness_args() -> WitnessHarnessArgs {
    let mut parsed = WitnessHarnessArgs::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => parsed.list = true,
            "--exact" => parsed.exact = true,
            "--nocapture" | "--quiet" | "-q" | "--show-output" | "--ignored"
            | "--include-ignored" => {}
            "--test-threads" | "--skip" | "--format" | "--color" => {
                let _ = args.next();
            }
            _ if arg.starts_with("--test-threads=")
                || arg.starts_with("--skip=")
                || arg.starts_with("--format=")
                || arg.starts_with("--color=") => {}
            _ if arg.starts_with('-') => {}
            _ if parsed.filter.is_none() => parsed.filter = Some(arg),
            _ => {}
        }
    }
    parsed
}

#[derive(Debug, Default)]
struct WitnessHarnessArgs {
    filter: Option<String>,
    exact: bool,
    list: bool,
}

impl WitnessHarnessArgs {
    fn matches(&self, name: &str) -> bool {
        let Some(filter) = &self.filter else {
            return true;
        };
        if self.exact {
            name == filter
        } else {
            name.contains(filter)
        }
    }
}

pub fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "dynamic witness test panicked with a non-string payload".to_owned()
    }
}

fn collect_dynamic_witness_tests() -> Vec<DynamicTestCase> {
    let mut tests = nirvash::inventory::iter::<RegisteredCodeWitnessTestProvider>
        .into_iter()
        .flat_map(|provider| (provider.build)())
        .collect::<Vec<_>>();
    tests.sort_by(|left, right| left.name.cmp(&right.name));
    tests
}

/// Runs all registered witness test providers with a small libtest-compatible CLI surface.
pub fn run_registered_code_witness_tests() {
    let args = parse_witness_harness_args();
    let tests = collect_dynamic_witness_tests();
    let selected = tests
        .into_iter()
        .filter(|test| args.matches(test.name()))
        .collect::<Vec<_>>();

    if args.list {
        for test in &selected {
            println!("{}: test", test.name());
        }
        println!();
        println!("{} tests, 0 benchmarks", selected.len());
        return;
    }

    println!("running {} tests", selected.len());
    let mut failures = Vec::new();
    for test in &selected {
        print!("test {} ... ", test.name());
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| test.execute()));
        match outcome {
            Ok(Ok(())) => println!("ok"),
            Ok(Err(message)) => {
                println!("FAILED");
                failures.push((test.name().to_owned(), message));
            }
            Err(payload) => {
                println!("FAILED");
                failures.push((test.name().to_owned(), panic_payload_to_string(payload)));
            }
        }
    }

    if failures.is_empty() {
        println!();
        println!(
            "test result: ok. {} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
            selected.len()
        );
        return;
    }

    println!();
    println!("failures:");
    for (name, message) in &failures {
        println!("---- {name} ----");
        println!("{message}");
        println!();
    }
    println!(
        "test result: FAILED. {} passed; {} failed; 0 ignored; 0 measured; 0 filtered out",
        selected.len().saturating_sub(failures.len()),
        failures.len()
    );
    process::exit(101);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash::StepExpr;
    use nirvash_lower::{
        ClaimedReduction, ProofObligation, ProofObligationKind, StateQuotientReduction,
        TemporalSpec,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum DemoAction {
        Advance,
        Branch,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum DemoState {
        Start,
        Next,
        Left,
        Right,
        Invalid,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct DeterministicDemoSpec;

    impl FrontendSpec for DeterministicDemoSpec {
        type State = DemoState;
        type Action = DemoAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![DemoState::Start]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![DemoAction::Advance]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (DemoState::Start, DemoAction::Advance) => Some(DemoState::Next),
                _ => None,
            }
        }
    }

    impl ProtocolConformanceSpec for DeterministicDemoSpec {
        type ExpectedOutput = ();
        type ProbeState = DemoState;
        type ProbeOutput = ();
        type SummaryState = DemoState;
        type SummaryOutput = ();

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            _next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
        }

        fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
            probe.clone()
        }

        fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
            *probe
        }

        fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
            summary.clone()
        }

        fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
            *summary
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct RelationDemoSpec;

    impl FrontendSpec for RelationDemoSpec {
        type State = DemoState;
        type Action = DemoAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![DemoState::Start]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![DemoAction::Branch]
        }

        fn transition_relation(
            &self,
            state: &Self::State,
            action: &Self::Action,
        ) -> Vec<Self::State> {
            match (state, action) {
                (DemoState::Start, DemoAction::Branch) => vec![DemoState::Left, DemoState::Right],
                _ => Vec::new(),
            }
        }
    }

    impl ProtocolConformanceSpec for RelationDemoSpec {
        type ExpectedOutput = ();
        type ProbeState = DemoState;
        type ProbeOutput = ();
        type SummaryState = DemoState;
        type SummaryOutput = ();

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            _next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
        }

        fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
            probe.clone()
        }

        fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
            *probe
        }

        fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
            summary.clone()
        }

        fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
            *summary
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct IdentityRefinementMap;

    impl RefinementMap<RelationDemoSpec> for IdentityRefinementMap {
        type ImplState = DemoState;
        type ImplInput = DemoAction;
        type ImplOutput = ();
        type AuxState = ();

        fn abstract_state(&self, state: &Self::ImplState, _aux: &Self::AuxState) -> DemoState {
            state.clone()
        }

        fn candidate_actions(
            &self,
            _before: &Self::ImplState,
            input: &Self::ImplInput,
            _output: &Self::ImplOutput,
            _after: &Self::ImplState,
            _aux: &Self::AuxState,
        ) -> Vec<DemoAction> {
            vec![input.clone()]
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TraceDemoAction {
        Advance,
        Reset,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TraceDemoState {
        Start,
        Left,
        Right,
        Done,
        Other,
        Bad,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TraceDemoOutput {
        Ack,
        Rejected,
        Wrong,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TraceDemoSpec;

    impl nirvash::FiniteModelDomain for TraceDemoState {
        fn finite_domain() -> nirvash::BoundedDomain<Self> {
            into_bounded_domain(vec![
                Self::Start,
                Self::Left,
                Self::Right,
                Self::Done,
                Self::Other,
                Self::Bad,
            ])
        }
    }

    impl nirvash::FiniteModelDomain for TraceDemoAction {
        fn finite_domain() -> nirvash::BoundedDomain<Self> {
            into_bounded_domain(vec![Self::Advance, Self::Reset])
        }
    }

    impl FrontendSpec for TraceDemoSpec {
        type State = TraceDemoState;
        type Action = TraceDemoAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![TraceDemoState::Start]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![TraceDemoAction::Advance, TraceDemoAction::Reset]
        }

        fn transition_relation(
            &self,
            state: &Self::State,
            action: &Self::Action,
        ) -> Vec<Self::State> {
            match (state, action) {
                (TraceDemoState::Start, TraceDemoAction::Advance) => {
                    vec![TraceDemoState::Left, TraceDemoState::Right]
                }
                (TraceDemoState::Left, TraceDemoAction::Advance) => vec![TraceDemoState::Done],
                (TraceDemoState::Right, TraceDemoAction::Advance) => vec![TraceDemoState::Other],
                (TraceDemoState::Done, TraceDemoAction::Reset)
                | (TraceDemoState::Other, TraceDemoAction::Reset) => vec![TraceDemoState::Start],
                _ => Vec::new(),
            }
        }
    }

    impl TemporalSpec for TraceDemoSpec {
        fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl ProtocolConformanceSpec for TraceDemoSpec {
        type ExpectedOutput = TraceDemoOutput;
        type ProbeState = TraceDemoState;
        type ProbeOutput = TraceDemoOutput;
        type SummaryState = TraceDemoState;
        type SummaryOutput = TraceDemoOutput;

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            if next.is_some() {
                TraceDemoOutput::Ack
            } else {
                TraceDemoOutput::Rejected
            }
        }

        fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
            probe.clone()
        }

        fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
            probe.clone()
        }

        fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
            summary.clone()
        }

        fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
            summary.clone()
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TemporalTraceSpec;

    impl FrontendSpec for TemporalTraceSpec {
        type State = TraceDemoState;
        type Action = TraceDemoAction;

        fn initial_states(&self) -> Vec<Self::State> {
            TraceDemoSpec.initial_states()
        }

        fn actions(&self) -> Vec<Self::Action> {
            TraceDemoSpec.actions()
        }

        fn transition_relation(
            &self,
            state: &Self::State,
            action: &Self::Action,
        ) -> Vec<Self::State> {
            TraceDemoSpec.transition_relation(state, action)
        }
    }

    impl TemporalSpec for TemporalTraceSpec {
        fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<nirvash::Ltl<Self::State, Self::Action>> {
            vec![nirvash::Ltl::eventually(nirvash::Ltl::pred(
                nirvash::BoolExpr::builtin_pure_call(
                    "eventually_done",
                    |state: &TraceDemoState| matches!(state, TraceDemoState::Done),
                ),
            ))]
        }
    }

    impl ProtocolConformanceSpec for TemporalTraceSpec {
        type ExpectedOutput = TraceDemoOutput;
        type ProbeState = TraceDemoState;
        type ProbeOutput = TraceDemoOutput;
        type SummaryState = TraceDemoState;
        type SummaryOutput = TraceDemoOutput;

        fn expected_output(
            &self,
            prev: &Self::State,
            action: &Self::Action,
            next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            TraceDemoSpec.expected_output(prev, action, next)
        }

        fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
            TraceDemoSpec.summarize_state(probe)
        }

        fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
            TraceDemoSpec.summarize_output(probe)
        }

        fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
            TraceDemoSpec.abstract_state(summary)
        }

        fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
            TraceDemoSpec.abstract_output(summary)
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct InvocationTraceMap;

    impl TraceRefinementMap<TraceDemoSpec> for InvocationTraceMap {
        type ImplState = TraceDemoState;
        type ImplInput = TraceDemoAction;
        type ImplOutput = TraceDemoOutput;
        type AuxState = Option<TraceDemoAction>;

        fn init_aux(&self, _initial: &Self::ImplState) -> Self::AuxState {
            None
        }

        fn next_aux(
            &self,
            _before: &Self::ImplState,
            event: &ObservedEvent<TraceDemoAction, Self::ImplOutput, Self::ImplInput>,
            _after: &Self::ImplState,
            aux: &Self::AuxState,
        ) -> Self::AuxState {
            match event {
                ObservedEvent::Invoke { input } => Some(input.clone()),
                ObservedEvent::Return { .. } | ObservedEvent::Action { .. } => None,
                ObservedEvent::Internal | ObservedEvent::Stutter => aux.clone(),
            }
        }

        fn abstract_state(&self, state: &Self::ImplState, _aux: &Self::AuxState) -> TraceDemoState {
            state.clone()
        }

        fn candidate_actions(
            &self,
            _before: &Self::ImplState,
            event: &ObservedEvent<TraceDemoAction, Self::ImplOutput, Self::ImplInput>,
            _after: &Self::ImplState,
            aux: &Self::AuxState,
        ) -> Vec<TraceDemoAction> {
            match event {
                ObservedEvent::Invoke { input } => vec![input.clone()],
                ObservedEvent::Return { .. } => aux.iter().cloned().collect(),
                ObservedEvent::Action { action, .. } => vec![action.clone()],
                ObservedEvent::Internal | ObservedEvent::Stutter => Vec::new(),
            }
        }

        fn output_matches(
            &self,
            spec: &TraceDemoSpec,
            action: &TraceDemoAction,
            abstract_before: &TraceDemoState,
            abstract_after: &TraceDemoState,
            event: &ObservedEvent<TraceDemoAction, Self::ImplOutput, Self::ImplInput>,
            _aux: &Self::AuxState,
        ) -> bool {
            match event {
                ObservedEvent::Return { output } | ObservedEvent::Action { output, .. } => {
                    spec.expected_output(abstract_before, action, Some(abstract_after)) == *output
                }
                ObservedEvent::Invoke { .. } => true,
                ObservedEvent::Internal | ObservedEvent::Stutter => {
                    abstract_before == abstract_after
                }
            }
        }

        fn hidden_step(
            &self,
            _before: &Self::ImplState,
            event: &ObservedEvent<TraceDemoAction, Self::ImplOutput, Self::ImplInput>,
            _after: &Self::ImplState,
            _aux: &Self::AuxState,
        ) -> bool {
            matches!(
                event,
                ObservedEvent::Invoke { .. } | ObservedEvent::Internal
            )
        }
    }

    fn left_terminal_trace() -> Trace<TraceDemoState, TraceDemoAction> {
        Trace::new(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                TraceStep::Action(TraceDemoAction::Advance),
                TraceStep::Action(TraceDemoAction::Advance),
                TraceStep::Stutter,
            ],
            2,
        )
    }

    fn reset_lasso_trace() -> Trace<TraceDemoState, TraceDemoAction> {
        Trace::new(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                TraceStep::Action(TraceDemoAction::Advance),
                TraceStep::Action(TraceDemoAction::Advance),
                TraceStep::Action(TraceDemoAction::Reset),
            ],
            0,
        )
    }

    fn trace_demo_model_case_with_obligation() -> ModelInstance<TraceDemoState, TraceDemoAction> {
        ModelInstance::new("trace_demo").with_claimed_reduction(
            ClaimedReduction::new().with_quotient(
                nirvash_lower::ReductionClaim::new(StateQuotientReduction::new(
                    "identity_quotient",
                    |state: &TraceDemoState| format!("{state:?}"),
                ))
                .with_obligation(ProofObligation::new(
                    "identity_quotient_sound".to_owned(),
                    ProofObligationKind::StateQuotientReduction,
                    "THEOREM identity_quotient_sound == QuotientSound".to_owned(),
                    "(assert QuotientSound)".to_owned(),
                )),
            ),
        )
    }

    #[test]
    fn step_refines_relation_succeeds_for_deterministic_summary_projection() {
        let spec = DeterministicDemoSpec;
        let map = SummaryRefinementMap(&spec);
        let witness = step_refines_relation(
            &spec,
            &map,
            &DemoState::Start,
            &DemoAction::Advance,
            &(),
            &DemoState::Next,
            &(),
        )
        .expect("deterministic step should refine");

        assert_eq!(
            witness,
            StepRefinementWitness {
                abstract_before: DemoState::Start,
                chosen_action: DemoAction::Advance,
                abstract_after: DemoState::Next,
            }
        );
        assert_eq!(witness.abstract_after, DemoState::Next);
    }

    #[test]
    fn step_refines_relation_accepts_multi_successor_transition() {
        let witness = step_refines_relation(
            &RelationDemoSpec,
            &IdentityRefinementMap,
            &DemoState::Start,
            &DemoAction::Branch,
            &(),
            &DemoState::Right,
            &(),
        )
        .expect("relation-based refinement should accept allowed successor");

        assert_eq!(
            witness,
            StepRefinementWitness {
                abstract_before: DemoState::Start,
                chosen_action: DemoAction::Branch,
                abstract_after: DemoState::Right,
            }
        );
    }

    #[test]
    fn enabled_from_summary_accepts_multi_successor_transition() {
        assert!(enabled_from_summary(
            &RelationDemoSpec,
            &DemoState::Start,
            &DemoAction::Branch,
        ));
    }

    #[test]
    fn step_refines_summary_returns_relation_witness() {
        let witness = step_refines_summary(
            &DeterministicDemoSpec,
            &DemoState::Start,
            &DemoAction::Advance,
            &DemoState::Next,
        )
        .expect("summary-based helper should refine deterministic transition");

        assert_eq!(
            witness,
            StepRefinementWitness {
                abstract_before: DemoState::Start,
                chosen_action: DemoAction::Advance,
                abstract_after: DemoState::Next,
            }
        );
    }

    #[test]
    fn step_refines_summary_reports_mismatch() {
        let error = step_refines_summary(
            &DeterministicDemoSpec,
            &DemoState::Start,
            &DemoAction::Advance,
            &DemoState::Invalid,
        )
        .expect_err("summary-based helper should reject mismatched target state");

        assert_eq!(
            error,
            StepRefinementError::NoMatchingAbstractSuccessor {
                abstract_before: DemoState::Start,
                abstract_after: DemoState::Invalid,
                candidate_actions: vec![DemoAction::Advance],
            }
        );
    }

    #[test]
    fn observed_terminal_trace_helper_refines_finite_prefix() {
        let observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
            ],
        );

        let witness = trace_refines_summary(&TraceDemoSpec, &observed, &left_terminal_trace())
            .expect("terminal helper should produce a refinement-compatible trace");

        assert_eq!(observed.loop_start(), Some(2));
        assert_eq!(witness.model_case_label, "direct");
        assert_eq!(witness.steps.len(), 3);
        assert!(matches!(
            witness.steps.last(),
            Some(TraceStepRefinementWitness {
                step: TraceStep::Stutter,
                ..
            })
        ));
    }

    #[test]
    fn trace_refines_summary_accepts_full_lasso() {
        let observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Left),
                StateObservation::Full(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Reset,
                    output: TraceDemoOutput::Ack,
                },
            ],
            Some(0),
        );

        let witness = trace_refines_summary(&TraceDemoSpec, &observed, &reset_lasso_trace())
            .expect("explicit lasso should refine");

        assert_eq!(witness.abstract_trace.loop_start(), 0);
        assert_eq!(
            witness.steps[2].step,
            TraceStep::Action(TraceDemoAction::Reset)
        );
        assert_eq!(witness.steps[2].abstract_after, TraceDemoState::Start);
    }

    #[test]
    fn trace_refines_summary_reports_loop_start_mismatch() {
        let observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Left),
                StateObservation::Full(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Stutter,
            ],
            Some(0),
        );

        let error = trace_refines_summary(&TraceDemoSpec, &observed, &left_terminal_trace())
            .expect_err("loop_start mismatch should fail");

        assert!(matches!(
            error,
            TraceRefinementError::ShapeMismatch { detail, .. }
                if detail.contains("loop_start")
        ));
    }

    #[test]
    fn trace_refines_summary_reports_output_mismatch() {
        let observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Wrong),
            ],
        );

        let error = trace_refines_summary(&TraceDemoSpec, &observed, &left_terminal_trace())
            .expect_err("output mismatch should fail");

        assert!(matches!(
            error,
            TraceRefinementError::StepMismatch {
                index,
                matching_prefix_len,
                detail,
                ..
            } if index == 1
                && matching_prefix_len == 1
                && detail.contains("expected output")
        ));
    }

    #[test]
    fn trace_refines_summary_rejects_action_event_when_candidate_stutters() {
        let observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Left),
                StateObservation::Full(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Reset,
                    output: TraceDemoOutput::Ack,
                },
            ],
            Some(2),
        );

        let error = trace_refines_summary(&TraceDemoSpec, &observed, &left_terminal_trace())
            .expect_err("candidate stutter should reject action event");

        assert!(matches!(
            error,
            TraceRefinementError::StutterMismatch { index, detail, .. }
                if index == 2 && detail.contains("does not match candidate stutter")
        ));
    }

    #[test]
    fn trace_refines_finds_matching_explicit_candidate() {
        let observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
            ],
        );

        let witness = trace_refines(&TraceDemoSpec, ModelInstance::new("trace_demo"), &observed)
            .expect("explicit search should find matching abstract trace");

        assert_eq!(witness.model_case_label, "trace_demo");
        assert_eq!(witness.abstract_trace, left_terminal_trace());
    }

    #[test]
    fn trace_refines_respects_model_case_action_constraints() {
        let observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
            ],
        );
        let model_case =
            ModelInstance::new("reset_only").with_action_constraint(StepExpr::builtin_pure_call(
                "only_reset",
                |_prev: &TraceDemoState, action: &TraceDemoAction, _next: &TraceDemoState| {
                    matches!(action, TraceDemoAction::Reset)
                },
            ));

        let error = trace_refines(&TraceDemoSpec, model_case, &observed)
            .expect_err("model case should filter out candidate actions");

        assert!(matches!(
            error,
            TraceRefinementError::NoMatchingExplicitCandidate {
                model_case_label,
                ..
            } if model_case_label == "reset_only"
        ));
    }

    #[test]
    fn trace_refines_returns_longest_matching_prefix_failure() {
        let observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Bad,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
            ],
        );

        let error = trace_refines(&TraceDemoSpec, ModelInstance::new("trace_demo"), &observed)
            .expect_err("no abstract candidate should match bad suffix");

        assert!(matches!(
            error,
            TraceRefinementError::NoMatchingExplicitCandidate {
                matching_prefix_len,
                failure,
                ..
            } if matching_prefix_len == 1
                && matches!(
                    *failure,
                    TraceRefinementError::StepMismatch { index, .. } if index == 1
                )
        ));
    }

    #[test]
    fn explicit_constrained_accepts_partial_and_unknown_state_observations() {
        let spec = TraceDemoSpec;
        let observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Unknown,
                StateObservation::Partial(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Stutter,
            ],
            Some(2),
        );

        let witness = constrained_trace_refines(
            &spec,
            &SummaryRefinementMap(&spec),
            ModelInstance::new("partial_unknown"),
            &observed,
            TraceRefinementConfig {
                engine: TraceRefinementEngine::ExplicitConstrained,
                max_hidden_steps_between_observations: 0,
                require_total_observation: false,
                allow_lasso: true,
            },
        )
        .expect("explicit constrained engine should tolerate partial and unknown observations");

        assert_eq!(witness.abstract_trace, left_terminal_trace());
    }

    #[test]
    fn explicit_constrained_accepts_invoke_return_and_internal_events() {
        let spec = TraceDemoSpec;
        let observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Left),
                StateObservation::Full(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Invoke {
                    input: TraceDemoAction::Advance,
                },
                ObservedEvent::Internal,
                ObservedEvent::Return {
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Stutter,
            ],
            Some(4),
        );

        let witness = constrained_trace_refines(
            &spec,
            &InvocationTraceMap,
            ModelInstance::new("invoke_return"),
            &observed,
            TraceRefinementConfig {
                engine: TraceRefinementEngine::ExplicitConstrained,
                max_hidden_steps_between_observations: 2,
                require_total_observation: true,
                allow_lasso: true,
            },
        )
        .expect("explicit constrained engine should admit invoke/internal/return refinement");

        assert_eq!(witness.abstract_trace, left_terminal_trace());
    }

    #[test]
    fn explicit_constrained_reports_hidden_step_budget_exhaustion() {
        let spec = TraceDemoSpec;
        let observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Left),
                StateObservation::Full(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Invoke {
                    input: TraceDemoAction::Advance,
                },
                ObservedEvent::Internal,
                ObservedEvent::Return {
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Stutter,
            ],
            Some(4),
        );

        let error = constrained_trace_refines(
            &spec,
            &InvocationTraceMap,
            ModelInstance::new("hidden_budget"),
            &observed,
            TraceRefinementConfig {
                engine: TraceRefinementEngine::ExplicitConstrained,
                max_hidden_steps_between_observations: 1,
                require_total_observation: true,
                allow_lasso: true,
            },
        )
        .expect_err("hidden step budget should fail closed");

        assert!(matches!(
            error,
            TraceRefinementError::NoMatchingExplicitCandidate {
                matching_prefix_len, ..
            } if matching_prefix_len == 0
        ));
    }

    #[test]
    fn explicit_constrained_accepts_lasso_observation() {
        let spec = TraceDemoSpec;
        let observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Left),
                StateObservation::Full(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Reset,
                    output: TraceDemoOutput::Ack,
                },
            ],
            Some(0),
        );

        let witness = constrained_trace_refines(
            &spec,
            &SummaryRefinementMap(&spec),
            ModelInstance::new("explicit_lasso"),
            &observed,
            TraceRefinementConfig {
                engine: TraceRefinementEngine::ExplicitConstrained,
                max_hidden_steps_between_observations: 0,
                require_total_observation: true,
                allow_lasso: true,
            },
        )
        .expect("explicit constrained engine should accept matching lasso");

        assert_eq!(witness.abstract_trace, reset_lasso_trace());
    }

    #[test]
    fn symbolic_constrained_accepts_finite_prefix_safety_trace() {
        let spec = TraceDemoSpec;
        let observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
            ],
        );

        let witness = constrained_trace_refines(
            &spec,
            &SummaryRefinementMap(&spec),
            ModelInstance::new("symbolic_prefix"),
            &observed,
            TraceRefinementConfig {
                engine: TraceRefinementEngine::SymbolicConstrained,
                max_hidden_steps_between_observations: 0,
                require_total_observation: true,
                allow_lasso: false,
            },
        )
        .expect("symbolic constrained engine should accept finite-prefix safety traces");

        assert_eq!(witness.abstract_trace, left_terminal_trace());
    }

    #[test]
    fn symbolic_constrained_rejects_lasso_and_temporal_observations() {
        let lasso_observed = ObservedTrace::new(
            vec![
                StateObservation::Full(TraceDemoState::Start),
                StateObservation::Full(TraceDemoState::Left),
                StateObservation::Full(TraceDemoState::Done),
            ],
            vec![
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Advance,
                    output: TraceDemoOutput::Ack,
                },
                ObservedEvent::Action {
                    action: TraceDemoAction::Reset,
                    output: TraceDemoOutput::Ack,
                },
            ],
            Some(0),
        );
        let lasso_error = constrained_trace_refines(
            &TraceDemoSpec,
            &SummaryRefinementMap(&TraceDemoSpec),
            ModelInstance::new("symbolic_lasso"),
            &lasso_observed,
            TraceRefinementConfig {
                engine: TraceRefinementEngine::SymbolicConstrained,
                max_hidden_steps_between_observations: 0,
                require_total_observation: true,
                allow_lasso: true,
            },
        )
        .expect_err("symbolic constrained engine should fail closed on lasso observations");

        assert!(matches!(
            lasso_error,
            TraceRefinementError::ShapeMismatch { detail, .. }
                if detail.contains("lasso")
        ));

        let temporal_observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
            ],
        );
        let temporal_error = constrained_trace_refines(
            &TemporalTraceSpec,
            &SummaryRefinementMap(&TemporalTraceSpec),
            ModelInstance::new("symbolic_temporal"),
            &temporal_observed,
            TraceRefinementConfig {
                engine: TraceRefinementEngine::SymbolicConstrained,
                max_hidden_steps_between_observations: 0,
                require_total_observation: true,
                allow_lasso: false,
            },
        )
        .expect_err("symbolic constrained engine should reject temporal fragments");

        assert!(matches!(
            temporal_error,
            TraceRefinementError::ShapeMismatch { detail, .. }
                if detail.contains("finite-prefix safety")
        ));
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(8))]
        #[test]
        fn proptest_adapter_smoke(branch_right in proptest::bool::ANY) {
            let observed = if branch_right {
                ObservedTrace::terminal(
                    vec![
                        TraceDemoState::Start,
                        TraceDemoState::Right,
                        TraceDemoState::Other,
                    ],
                    vec![
                        (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                        (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                    ],
                )
            } else {
                ObservedTrace::terminal(
                    vec![
                        TraceDemoState::Start,
                        TraceDemoState::Left,
                        TraceDemoState::Done,
                    ],
                    vec![
                        (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                        (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                    ],
                )
            };
            let model_case = trace_demo_model_case_with_obligation();

            let witness = proptest_adapter::assert_input_sequence_refines(
                &TraceDemoSpec,
                model_case.clone(),
                &observed,
            )
            .expect("proptest adapter should refine observed trace");

            proptest::prop_assert_eq!(witness.model_case_label, "trace_demo");
            proptest::prop_assert_eq!(
                proptest_adapter::expected_obligations(&model_case)
                    .into_iter()
                    .map(|obligation| obligation.kind)
                    .collect::<Vec<_>>(),
                vec![ProofObligationKind::StateQuotientReduction]
            );
        }
    }

    #[test]
    fn loom_adapter_smoke() {
        let observed = ObservedTrace::terminal(
            vec![
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
            ],
            vec![
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
                (TraceDemoAction::Advance, TraceDemoOutput::Ack),
            ],
        );
        let model_case = trace_demo_model_case_with_obligation();
        let witness =
            loom_adapter::assert_schedule_refines(&TraceDemoSpec, model_case.clone(), &observed)
                .expect("loom adapter should refine observed schedule");

        assert_eq!(witness.model_case_label, "trace_demo");
        assert_eq!(
            loom_adapter::expected_obligations(&model_case)
                .into_iter()
                .map(|obligation| obligation.kind)
                .collect::<Vec<_>>(),
            vec![ProofObligationKind::StateQuotientReduction]
        );
    }

    #[test]
    fn step_refines_relation_reports_abstract_successor_mismatch() {
        let error = step_refines_relation(
            &RelationDemoSpec,
            &IdentityRefinementMap,
            &DemoState::Start,
            &DemoAction::Branch,
            &(),
            &DemoState::Invalid,
            &(),
        )
        .expect_err("invalid abstract successor should be rejected");

        assert_eq!(
            error,
            StepRefinementError::NoMatchingAbstractSuccessor {
                abstract_before: DemoState::Start,
                abstract_after: DemoState::Invalid,
                candidate_actions: vec![DemoAction::Branch],
            }
        );
    }
}
