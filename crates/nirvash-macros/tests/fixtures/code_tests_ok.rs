use std::sync::Mutex;

use nirvash_lower::FrontendSpec;
use nirvash_conformance::{
    ActionApplier, ProtocolConformanceSpec, ProtocolRuntimeBinding, StateObserver,
};
use nirvash_macros::FiniteModelDomain as FormalFiniteModelDomain;
use nirvash_macros::code_tests;

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

#[derive(Clone, Copy, Debug, Default)]
struct Context;

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

    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        self.actions()
            .into_iter()
            .filter_map(|action| self.transition(state, &action).map(|next| (action, next)))
            .collect()
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

#[code_tests(spec = Spec, binding = Binding)]
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

fn main() {}
