use std::{fmt::Debug, panic::AssertUnwindSafe, process};

pub use crate::ReachableGraphSnapshot;
pub use crate::system::{
    ActionApplier, ModelCase, ModelCaseSource, StateObserver, TransitionSystem,
};
use crate::{IntoBoundedDomain, into_bounded_domain};

/// Spec-side contract for replaying runtime behavior against a transition system.
pub trait ProtocolConformanceSpec: TransitionSystem {
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

/// Concrete input that should follow a valid abstract transition.
#[derive(Debug, Clone)]
pub struct PositiveWitness<Context, Input> {
    name: String,
    context: Context,
    input: Input,
    canonical: bool,
}

impl<Context, Input> PositiveWitness<Context, Input> {
    /// Creates a named positive witness with concrete context and input.
    pub fn new(name: impl Into<String>, context: Context, input: Input) -> Self {
        Self {
            name: name.into(),
            context,
            input,
            canonical: false,
        }
    }

    /// Marks whether this witness is the canonical replay choice for prefix execution.
    pub fn with_canonical(mut self, canonical: bool) -> Self {
        self.canonical = canonical;
        self
    }

    /// Returns the stable witness label used in test names and failures.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the concrete runtime context for the witness.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Returns the concrete runtime input for the witness.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// Returns whether this witness is used for canonical prefix replay.
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
    /// Creates a named negative witness with concrete context and input.
    pub fn new(name: impl Into<String>, context: Context, input: Input) -> Self {
        Self {
            name: name.into(),
            context,
            input,
        }
    }

    /// Returns the stable witness label used in test names and failures.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the concrete runtime context for the witness.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Returns the concrete runtime input for the witness.
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
    /// Encodes the canonical concrete input used for an allowed abstract transition.
    fn canonical_positive(action: &Action) -> Self;

    /// Returns the concrete inputs used for allowed abstract transitions.
    ///
    /// Implementations should place the canonical witness at index `0`.
    fn positive_family(action: &Action) -> Vec<Self> {
        vec![Self::canonical_positive(action)]
    }

    /// Returns the concrete inputs used for rejected abstract transitions.
    fn negative_family(action: &Action) -> Vec<Self> {
        vec![Self::canonical_positive(action)]
    }

    /// Returns the stable witness label used by generated witnesses.
    fn witness_name(_action: &Action, kind: WitnessKind, index: usize) -> String {
        match kind {
            WitnessKind::CanonicalPositive => "principal".to_owned(),
            WitnessKind::Positive => format!("positive_{index}"),
            WitnessKind::Negative => format!("negative_{index}"),
        }
    }
}

/// Borrowed witness family used by helper validation.
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

    /// Creates a fresh session that can carry probe identity or other witness-local state.
    async fn fresh_session(spec: &Spec) -> Self::Session;

    /// Returns the concrete inputs that should realize a valid abstract transition.
    fn positive_witnesses(
        spec: &Spec,
        session: &Self::Session,
        prev: &Spec::State,
        action: &Spec::Action,
        next: &Spec::State,
    ) -> Vec<PositiveWitness<Self::Context, Self::Input>>;

    /// Returns the concrete inputs that should keep the abstract transition rejected.
    fn negative_witnesses(
        spec: &Spec,
        session: &Self::Session,
        prev: &Spec::State,
        action: &Spec::Action,
    ) -> Vec<NegativeWitness<Self::Context, Self::Input>>;

    /// Executes a concrete witness input against the runtime.
    async fn execute_input(
        runtime: &Self::Runtime,
        session: &mut Self::Session,
        context: &Self::Context,
        input: &Self::Input,
    ) -> Spec::ProbeOutput;

    /// Returns the probe context used to observe the authoritative runtime state.
    fn probe_context(session: &Self::Session) -> Self::Context;
}

/// Dynamically built test case used by the witness harness.
pub struct DynamicTestCase {
    name: String,
    run: Box<dyn Fn() -> Result<(), String>>,
}

impl DynamicTestCase {
    /// Creates a dynamically named test case.
    pub fn new<F>(name: impl Into<String>, run: F) -> Self
    where
        F: Fn() -> Result<(), String> + 'static,
    {
        Self {
            name: name.into(),
            run: Box::new(run),
        }
    }

    /// Returns the externally visible test name.
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

crate::inventory::collect!(RegisteredCodeWitnessTestProvider);

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
    <Spec as TransitionSystem>::transition(spec, &projected, action).is_some()
}

pub fn assert_initial_refinement<Spec>(spec: &Spec, summary: &Spec::SummaryState)
where
    Spec: ProtocolConformanceSpec,
    Spec::State: PartialEq,
{
    let projected = spec.abstract_state(summary);
    let initial_states = <Spec as TransitionSystem>::initial_states(spec);
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
    let before = spec.abstract_state(before_summary);
    let expected_next = <Spec as TransitionSystem>::transition(spec, &before, action)
        .expect("step refinement requires an allowed abstract transition");
    let projected_after = spec.abstract_state(after_summary);
    assert_eq!(
        projected_after, expected_next,
        "summary/state next mismatch for {action:?} from {before_summary:?}",
    );
    expected_next
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

pub fn assert_witness_family_completeness<Context, Input>(
    label: &str,
    family: WitnessFamily<'_, Context, Input>,
) where
    Context: Debug,
    Input: Debug,
{
    match family {
        WitnessFamily::Positive(witnesses) => {
            assert!(
                !witnesses.is_empty(),
                "{label}: positive witnesses are empty"
            );
            let canonical = witnesses
                .iter()
                .filter(|witness| witness.canonical())
                .count();
            assert_eq!(
                canonical,
                1,
                "{label}: expected canonical positive witness count = 1, found {} from {:?}",
                canonical,
                witnesses
                    .iter()
                    .map(|witness| format!("{}(canonical={})", witness.name(), witness.canonical()))
                    .collect::<Vec<_>>(),
            );
        }
        WitnessFamily::Negative(witnesses) => {
            assert!(
                !witnesses.is_empty(),
                "{label}: negative witnesses are empty"
            );
        }
    }
}

pub fn assert_witness_codec_exhaustive<Action, Input, Domain, Canonical, Positive, Negative, Name>(
    label: &str,
    domain: Domain,
    canonical_positive: Canonical,
    positive_family: Positive,
    negative_family: Negative,
    witness_name: Name,
) where
    Action: Debug,
    Input: Debug + Clone,
    Domain: IntoBoundedDomain<Action>,
    Canonical: Fn(&Action) -> Input,
    Positive: Fn(&Action) -> Vec<Input>,
    Negative: Fn(&Action) -> Vec<Input>,
    Name: Fn(&Action, WitnessKind, usize) -> String,
{
    for action in into_bounded_domain(domain).into_vec() {
        let _canonical = canonical_positive(&action);
        let positive = positive_family(&action);
        let negative = negative_family(&action);
        assert!(
            !positive.is_empty(),
            "{label}: positive witness codec family is empty for action {action:?}",
        );
        assert!(
            !negative.is_empty(),
            "{label}: negative witness codec family is empty for action {action:?}",
        );
        let positive_names = positive
            .iter()
            .enumerate()
            .map(|(index, _)| {
                witness_name(
                    &action,
                    if index == 0 {
                        WitnessKind::CanonicalPositive
                    } else {
                        WitnessKind::Positive
                    },
                    index,
                )
            })
            .collect::<Vec<_>>();
        let negative_names = negative
            .iter()
            .enumerate()
            .map(|(index, _)| witness_name(&action, WitnessKind::Negative, index))
            .collect::<Vec<_>>();
        assert!(
            positive_names.iter().all(|name| !name.is_empty()),
            "{label}: positive witness codec produced an empty name for action {action:?}",
        );
        assert!(
            negative_names.iter().all(|name| !name.is_empty()),
            "{label}: negative witness codec produced an empty name for action {action:?}",
        );
    }
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
    let mut tests = crate::inventory::iter::<RegisteredCodeWitnessTestProvider>
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
    use crate::ActionVocabulary;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DummyAction {
        Advance,
        Reject,
    }

    impl ActionVocabulary for DummyAction {
        fn action_vocabulary() -> Vec<Self> {
            vec![Self::Advance, Self::Reject]
        }
    }

    #[derive(Debug, Default, Clone, Copy)]
    struct DummySpec;

    impl TransitionSystem for DummySpec {
        type State = bool;
        type Action = DummyAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![false]
        }

        fn actions(&self) -> Vec<Self::Action> {
            DummyAction::action_vocabulary()
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (false, DummyAction::Advance) => Some(true),
                _ => None,
            }
        }
    }

    impl ProtocolConformanceSpec for DummySpec {
        type ExpectedOutput = &'static str;
        type ProbeState = bool;
        type ProbeOutput = &'static str;
        type SummaryState = bool;
        type SummaryOutput = &'static str;

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            _next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            "ok"
        }

        fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
            *probe
        }

        fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
            *probe
        }

        fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
            *summary
        }

        fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
            *summary
        }
    }

    #[test]
    fn refinement_helpers_accept_allowed_step_and_output() {
        let spec = DummySpec;
        assert_initial_refinement(&spec, &false);
        let next = assert_step_refinement(&spec, &false, &DummyAction::Advance, &true);
        assert!(next);
        assert_output_refinement(&spec, &false, &DummyAction::Advance, &true, &"ok");
    }

    #[test]
    fn step_refinement_panics_for_rejected_transition() {
        let spec = DummySpec;
        let panic = std::panic::catch_unwind(|| {
            let _ = assert_step_refinement(&spec, &false, &DummyAction::Reject, &false);
        });
        assert!(panic.is_err(), "rejected transition should panic");
    }

    #[test]
    fn output_refinement_panics_for_mismatched_output() {
        let spec = DummySpec;
        let panic = std::panic::catch_unwind(|| {
            assert_output_refinement(&spec, &false, &DummyAction::Advance, &true, &"bad");
        });
        assert!(panic.is_err(), "mismatched output should panic");
    }

    #[test]
    fn enabled_from_summary_matches_transition_reachability() {
        let spec = DummySpec;

        assert!(enabled_from_summary(&spec, &false, &DummyAction::Advance));
        assert!(!enabled_from_summary(&spec, &false, &DummyAction::Reject));
        assert!(!enabled_from_summary(&spec, &true, &DummyAction::Advance));
    }

    #[test]
    fn declared_projection_helpers_accept_matching_values() {
        assert_declared_state_projection(&true, &true, &"state", &"state");
        assert_declared_output_projection(&vec!["ok"], &vec!["ok"]);
    }

    #[test]
    fn witness_family_helper_accepts_positive_and_negative_cases() {
        let positive =
            vec![PositiveWitness::new("principal", (), DummyAction::Advance).with_canonical(true)];
        let negative = vec![NegativeWitness::new("principal", (), DummyAction::Reject)];

        assert_witness_family_completeness("positive", WitnessFamily::Positive(&positive));
        assert_witness_family_completeness("negative", WitnessFamily::Negative(&negative));
    }

    #[test]
    fn exhaustive_helpers_accept_matching_domains() {
        assert_projection_exhaustive(
            "bool identity",
            [false, true],
            |value| !value,
            |value| !value,
        );
        assert_witness_codec_exhaustive(
            "dummy codec",
            [DummyAction::Advance, DummyAction::Reject],
            |action| *action,
            |action| vec![*action],
            |action| vec![*action],
            |_action, kind, index| match kind {
                WitnessKind::CanonicalPositive => format!("canonical_{index}"),
                WitnessKind::Positive => format!("positive_{index}"),
                WitnessKind::Negative => format!("negative_{index}"),
            },
        );
    }
}
