use std::sync::Mutex;

use nirvash_lower::FrontendSpec;
use nirvash_conformance::{
    ActionApplier, NegativeWitness, PositiveWitness, ProtocolConformanceSpec,
    ProtocolInputWitnessBinding, ProtocolRuntimeBinding, StateObserver,
};
use nirvash_macros::FiniteModelDomain as FormalFiniteModelDomain;
use nirvash_macros::{code_witness_test_main, code_witness_tests};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, Default)]
struct Binding;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Start,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Output {
    Ack,
    Rejected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Context;

#[derive(Clone, Copy, Debug, Default)]
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
        vec![Action::Start, Action::Stop]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Idle, Action::Start) => Some(State::Busy),
            (State::Busy, Action::Stop) => Some(State::Idle),
            _ => None,
        }
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
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        match (prev, action, next) {
            (State::Idle, Action::Start, Some(State::Busy))
            | (State::Busy, Action::Stop, Some(State::Idle)) => Output::Ack,
            _ => Output::Rejected,
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
    type Input = Action;
    type Session = Session;

    async fn fresh_session(_spec: &Spec) -> Self::Session {
        Session { context: Context }
    }

    fn positive_witnesses(
        _spec: &Spec,
        session: &Self::Session,
        _prev: &State,
        action: &Action,
        _next: &State,
    ) -> Vec<PositiveWitness<Self::Context, Self::Input>> {
        vec![PositiveWitness::new("principal", session.context, action.clone()).with_canonical(true)]
    }

    fn negative_witnesses(
        _spec: &Spec,
        session: &Self::Session,
        _prev: &State,
        action: &Action,
    ) -> Vec<NegativeWitness<Self::Context, Self::Input>> {
        vec![NegativeWitness::new(
            "principal",
            session.context,
            action.clone(),
        )]
    }

    async fn execute_input(
        runtime: &Self::Runtime,
        _session: &mut Self::Session,
        context: &Self::Context,
        input: &Self::Input,
    ) -> Output {
        runtime.execute_action(context, input).await
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
                *state = State::Busy;
                Output::Ack
            }
            (State::Busy, Action::Stop) => {
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

code_witness_test_main!();
