use std::sync::Mutex;

use nirvash::{BoolExpr, inventory};
use nirvash_conformance::{
    ActionApplier, NegativeWitness, PositiveWitness, ProtocolConformanceSpec,
    ProtocolInputWitnessBinding, ProtocolRuntimeBinding, RegisteredCodeWitnessTestProvider,
    StateObserver, run_registered_code_witness_tests,
};
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::FiniteModelDomain as FormalFiniteModelDomain;
use nirvash_macros::code_witness_tests;

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, Default)]
struct Binding;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
    Fast,
    Slow,
}

#[derive(Clone, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Start,
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Input {
    StartFast,
    StartSlow,
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Output {
    Ack,
    Rejected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Context;

#[derive(Clone, Debug, Default)]
struct Session {
    context: Context,
}

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn frontend_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start, Action::Reset]
    }

    fn transition_relation(&self, state: &Self::State, action: &Self::Action) -> Vec<Self::State> {
        match (state, action) {
            (State::Idle, Action::Start) => vec![State::Fast, State::Slow],
            (State::Fast | State::Slow, Action::Reset) => vec![State::Idle],
            _ => Vec::new(),
        }
    }
}

impl TemporalSpec for Spec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl ProtocolConformanceSpec for Spec {
    type ExpectedOutput = Output;
    type ProbeState = State;
    type ProbeOutput = Output;
    type SummaryState = State;
    type SummaryOutput = Output;

    fn expected_output(
        &self,
        _prev: &Self::State,
        _action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        if next.is_some() {
            Output::Ack
        } else {
            Output::Rejected
        }
    }

    fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
        *probe
    }

    fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
        probe.clone()
    }

    fn abstract_state(&self, observed: &Self::SummaryState) -> Self::State {
        *observed
    }

    fn abstract_output(&self, observed: &Self::SummaryOutput) -> Self::ExpectedOutput {
        observed.clone()
    }
}

#[code_witness_tests(spec = Spec, binding = Binding)]
const _: () = ();

struct Driver {
    state: Mutex<State>,
}

impl ProtocolRuntimeBinding<Spec> for Binding {
    type Runtime = Driver;
    type Context = Context;

    async fn fresh_runtime(_spec: &Spec) -> Self::Runtime {
        Driver {
            state: Mutex::new(State::Idle),
        }
    }

    fn context(_spec: &Spec) -> Self::Context {
        Context
    }
}

impl ProtocolInputWitnessBinding<Spec> for Binding {
    type Input = Input;
    type Session = Session;

    async fn fresh_session(_spec: &Spec) -> Self::Session {
        Session { context: Context }
    }

    fn positive_witnesses(
        _spec: &Spec,
        session: &Self::Session,
        prev: &State,
        action: &Action,
        next: &State,
    ) -> Vec<PositiveWitness<Self::Context, Self::Input>> {
        match (prev, action, next) {
            (State::Idle, Action::Start, State::Fast) => vec![
                PositiveWitness::new("start_fast", session.context, Input::StartFast)
                    .with_canonical(true),
            ],
            (State::Idle, Action::Start, State::Slow) => vec![
                PositiveWitness::new("start_slow", session.context, Input::StartSlow)
                    .with_canonical(true),
            ],
            (State::Fast, Action::Reset, State::Idle) => vec![
                PositiveWitness::new("reset_from_fast", session.context, Input::Reset)
                    .with_canonical(true),
            ],
            (State::Slow, Action::Reset, State::Idle) => vec![
                PositiveWitness::new("reset_from_slow", session.context, Input::Reset)
                    .with_canonical(true),
            ],
            _ => Vec::new(),
        }
    }

    fn negative_witnesses(
        _spec: &Spec,
        session: &Self::Session,
        prev: &State,
        action: &Action,
    ) -> Vec<NegativeWitness<Self::Context, Self::Input>> {
        let (name, input) = match (prev, action) {
            (State::Idle, Action::Reset) => ("reject_reset_from_idle", Input::Reset),
            (State::Fast, Action::Start) => ("reject_start_from_fast", Input::StartFast),
            (State::Slow, Action::Start) => ("reject_start_from_slow", Input::StartSlow),
            _ => ("reject", Input::Reset),
        };
        vec![NegativeWitness::new(name, session.context, input)]
    }

    async fn execute_input(
        runtime: &Self::Runtime,
        _session: &mut Self::Session,
        _context: &Self::Context,
        input: &Self::Input,
    ) -> Output {
        let mut state = runtime.state.lock().expect("lock state");
        match (*state, input) {
            (State::Idle, Input::StartFast) => {
                *state = State::Fast;
                Output::Ack
            }
            (State::Idle, Input::StartSlow) => {
                *state = State::Slow;
                Output::Ack
            }
            (State::Fast | State::Slow, Input::Reset) => {
                *state = State::Idle;
                Output::Ack
            }
            _ => Output::Rejected,
        }
    }

    fn probe_context(session: &Self::Session) -> Self::Context {
        session.context
    }
}

impl ActionApplier for Driver {
    type Action = Action;
    type Output = Output;
    type Context = Context;

    async fn execute_action(&self, _context: &Context, action: &Action) -> Output {
        let mut state = self.state.lock().expect("lock state");
        match (*state, action) {
            (State::Idle, Action::Start) => {
                *state = State::Fast;
                Output::Ack
            }
            (State::Fast | State::Slow, Action::Reset) => {
                *state = State::Idle;
                Output::Ack
            }
            _ => Output::Rejected,
        }
    }
}

impl StateObserver for Driver {
    type SummaryState = State;
    type Context = Context;

    async fn observe_state(&self, _context: &Context) -> State {
        *self.state.lock().expect("lock state")
    }
}

fn generated_test_names() -> Vec<String> {
    let mut names = inventory::iter::<RegisteredCodeWitnessTestProvider>
        .into_iter()
        .flat_map(|provider| (provider.build)())
        .map(|test| test.name().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn assert_nondeterministic_paths_registered() {
    let names = generated_test_names();

    assert!(
        names.iter().any(|name| {
            name.contains("/from_init/when_Start/")
                && name.contains("/start_fast-0")
                && name.contains("/via_")
        }),
        "missing fast successor witness name: {names:#?}"
    );
    assert!(
        names.iter().any(|name| {
            name.contains("/from_init/when_Start/")
                && name.contains("/start_slow-0")
                && name.contains("/via_")
        }),
        "missing slow successor witness name: {names:#?}"
    );
}

#[doc(hidden)]
pub fn __nirvash_code_witness_main_marker() {}

fn main() {
    assert_nondeterministic_paths_registered();
    run_registered_code_witness_tests();
}
