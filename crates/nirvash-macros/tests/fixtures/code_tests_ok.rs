use std::sync::Mutex;

use nirvash_core::{
    ActionApplier, CodeConformanceSpec, ExpectedStep, StateObserver, TransitionSystem,
};
use nirvash_macros::{Signature as FormalSignature, code_tests};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq, FormalSignature)]
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

impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn init(&self, state: &Self::State) -> bool {
        matches!(state, State::Idle)
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        match self.expected_step(prev, action) {
            ExpectedStep::Allowed { next: expected, .. } => expected == *next,
            ExpectedStep::Rejected { .. } => false,
        }
    }
}

impl CodeConformanceSpec for Spec {
    type Runtime = Driver;
    type Context = Context;
    type ExpectedOutput = Output;
    type ObservedState = State;
    type ObservedOutput = Output;

    async fn fresh_runtime(&self) -> Self::Runtime {
        Driver {
            state: Mutex::new(State::Idle),
        }
    }

    fn context(&self) -> Self::Context {
        Context
    }

    fn expected_step(
        &self,
        prev: &Self::State,
        action: &Self::Action,
    ) -> ExpectedStep<Self::State, Self::ExpectedOutput> {
        match (prev, action) {
            (State::Idle, Action::Start) => ExpectedStep::Allowed {
                next: State::Busy,
                output: Output::Ack,
            },
            (State::Busy, Action::Stop) => ExpectedStep::Allowed {
                next: State::Idle,
                output: Output::Ack,
            },
            _ => ExpectedStep::Rejected {
                output: Output::Rejected,
            },
        }
    }

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State {
        *observed
    }

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput {
        observed.clone()
    }
}

fn initial_state() -> State {
    State::Idle
}

#[code_tests(spec = Spec, init = initial_state)]
const _: () = ();

struct Driver {
    state: Mutex<State>,
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
    type ObservedState = State;
    type Context = Context;

    async fn observe_state(&self, _context: &Context) -> State {
        *self.state.lock().expect("lock state")
    }
}

fn main() {}
