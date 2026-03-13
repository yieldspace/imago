use std::{fmt::Debug, panic::AssertUnwindSafe, process};

use nirvash::{IntoBoundedDomain, into_bounded_domain};
pub use nirvash::{ReachableGraphSnapshot, inventory};
pub use nirvash_lower::{FrontendSpec, ModelInstance, Trace};

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
    <Spec as FrontendSpec>::transition(spec, &projected, action).is_some()
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
    let before = spec.abstract_state(before_summary);
    let expected_next = <Spec as FrontendSpec>::transition(spec, &before, action)
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
