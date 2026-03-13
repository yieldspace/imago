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
}

#[derive(Clone, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
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
        vec![Action::Tick]
    }

    fn transition(&self, _state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
        Some(State::Idle)
    }

    fn successors(&self, _state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        vec![(Action::Tick, State::Idle)]
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
        _next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        Output::Ack
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

    async fn execute_action(&self, _context: &Context, _action: &Action) -> Output {
        let _ = self.state.lock().expect("lock state");
        Output::Ack
    }
}

impl StateObserver for Driver {
    type SummaryState = State;
    type Context = Context;

    async fn observe_state(&self, _context: &Context) -> State {
        *self.state.lock().expect("lock state")
    }
}

#[code_tests(spec = Spec, binding = Binding, action = Action)]
const _: () = ();

fn main() {}
