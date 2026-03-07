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
}

#[derive(Clone, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Tick,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Output {
    Ack,
}

#[derive(Clone, Copy, Debug, Default)]
struct Context;

struct Driver {
    state: Mutex<State>,
}

impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn init(&self, state: &Self::State) -> bool {
        matches!(state, State::Idle)
    }

    fn next(&self, _prev: &Self::State, _action: &Self::Action, next: &Self::State) -> bool {
        matches!(next, State::Idle)
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
        _prev: &Self::State,
        _action: &Self::Action,
    ) -> ExpectedStep<Self::State, Self::ExpectedOutput> {
        ExpectedStep::Allowed {
            next: State::Idle,
            output: Output::Ack,
        }
    }

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State {
        *observed
    }

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput {
        observed.clone()
    }
}

impl ActionApplier for Driver {
    type Action = Action;
    type Output = Output;
    type Context = Context;

    async fn execute_action(&self, _context: &Context, _action: &Action) -> Output {
        let _ = self.state.lock().expect("lock state");
        Output::Ack
    }
}

impl StateObserver for Driver {
    type ObservedState = State;
    type Context = Context;

    async fn observe_state(&self, _context: &Context) -> State {
        *self.state.lock().expect("lock state")
    }
}

fn initial_state() -> State {
    State::Idle
}

#[code_tests(spec = Spec, init = initial_state, driver = tests::Driver)]
const _: () = ();

fn main() {}
