use std::sync::Mutex;

use nirvash::ActionVocabulary;
use nirvash_lower::FrontendSpec;
use nirvash_conformance::{ActionApplier, ProtocolConformanceSpec, StateObserver};
use nirvash_macros::{
    ActionVocabulary as FormalActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    code_witness_test_main, nirvash_runtime_contract,
};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, Default)]
struct Context;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalActionVocabulary)]
enum Action {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Output {
    Ack,
    #[default]
    Rejected,
}

#[derive(Debug, Default, Clone, Copy)]
struct Binding;

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
        Action::action_vocabulary()
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

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
        *summary
    }

    fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
        *summary
    }
}

impl ActionApplier for Driver {
    type Action = Action;
    type Output = Output;
    type Context = Context;

    async fn execute_action(&self, _context: &Self::Context, action: &Self::Action) -> Self::Output {
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

    async fn observe_state(&self, _context: &Self::Context) -> State {
        *self.state.lock().expect("lock state")
    }
}

#[nirvash_runtime_contract(
    spec = Spec,
    binding = Binding,
    runtime = Driver,
    context = Context,
    context_expr = Context,
    fresh_runtime = Driver {
        state: Mutex::new(State::Idle),
    },
    tests(witness)
)]
impl Binding {}

code_witness_test_main!();
