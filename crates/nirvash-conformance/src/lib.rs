use std::{fmt::Debug, panic::AssertUnwindSafe, process};

use nirvash::{IntoBoundedDomain, into_bounded_domain};
pub use nirvash::{ReachableGraphSnapshot, inventory};
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
pub enum ObservedEvent<A, SummaryOutput> {
    Action { action: A, output: SummaryOutput },
    Stutter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedTrace<SummaryState, A, SummaryOutput> {
    states: Vec<SummaryState>,
    events: Vec<ObservedEvent<A, SummaryOutput>>,
    loop_start: usize,
}

impl<SummaryState, A, SummaryOutput> ObservedTrace<SummaryState, A, SummaryOutput> {
    pub fn new(
        states: Vec<SummaryState>,
        events: Vec<ObservedEvent<A, SummaryOutput>>,
        loop_start: usize,
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
        let loop_start = states.len() - 1;
        Self::new(states, events, loop_start)
    }

    pub fn states(&self) -> &[SummaryState] {
        &self.states
    }

    pub fn events(&self) -> &[ObservedEvent<A, SummaryOutput>] {
        &self.events
    }

    pub const fn loop_start(&self) -> usize {
        self.loop_start
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn next_index(&self, index: usize) -> usize {
        if index + 1 < self.states.len() {
            index + 1
        } else {
            self.loop_start
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

pub fn assert_step_refinement<Spec>(
    spec: &Spec,
    before_summary: &Spec::SummaryState,
    action: &Spec::Action,
    after_summary: &Spec::SummaryState,
) -> Spec::State
where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
{
    step_refines_summary(spec, before_summary, action, after_summary)
        .unwrap_or_else(|error| {
            panic!("step refinement failed for {action:?} from {before_summary:?}: {error}")
        })
        .abstract_after
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
    if observed.events().len() != observed.states().len() {
        return Err(shape_error(format!(
            "observed trace has {} states but {} events",
            observed.states().len(),
            observed.events().len()
        )));
    }
    if observed.loop_start() >= observed.states().len() {
        return Err(shape_error(format!(
            "observed loop_start {} is outside state length {}",
            observed.loop_start(),
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
    if observed.loop_start() != abstract_trace.loop_start() {
        return Err(shape_error(format!(
            "observed loop_start {} does not match candidate loop_start {}",
            observed.loop_start(),
            abstract_trace.loop_start()
        )));
    }

    let observed_initial = spec.abstract_state(&observed.states()[0]);
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
        let observed_before = spec.abstract_state(&observed.states()[index]);
        let observed_after_summary = &observed.states()[observed.next_index(index)];
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
                    &observed.states()[index],
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
    use nirvash_lower::TemporalSpec;

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
        assert_eq!(
            assert_step_refinement(
                &spec,
                &DemoState::Start,
                &DemoAction::Advance,
                &DemoState::Next,
            ),
            DemoState::Next
        );
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

        assert_eq!(observed.loop_start(), 2);
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
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
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
            0,
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
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
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
            0,
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
                TraceDemoState::Start,
                TraceDemoState::Left,
                TraceDemoState::Done,
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
            2,
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
