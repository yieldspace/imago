use std::sync::Mutex;

use nirvash::ActionVocabulary;
use nirvash_lower::FrontendSpec;
use nirvash_conformance::{ActionApplier, ProtocolConformanceSpec};
use nirvash_macros::{
    ActionVocabulary as FormalActionVocabulary, ProtocolInputWitness, FiniteModelDomain as FormalFiniteModelDomain,
    code_witness_test_main, nirvash_runtime_contract,
};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, FormalFiniteModelDomain)]
enum State {
    #[default]
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

#[derive(Clone, Debug, PartialEq, Eq, ProtocolInputWitness)]
struct Input(Action);

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
        *probe
    }

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
        *summary
    }

    fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
        *summary
    }
}

#[derive(Debug, Default)]
struct Driver {
    state: Mutex<State>,
}

async fn observe_driver_state(runtime: &Driver, _context: &()) -> State {
    *runtime.state.lock().expect("lock state")
}

fn observe_driver_output(
    _runtime: &Driver,
    _context: &(),
    _action: &Action,
    _result: &Output,
) -> Output {
    Output::Ack
}

async fn dispatch_input(
    runtime: &Driver,
    _session: &mut (),
    context: &(),
    input: &Input,
) -> Output {
    <Driver as ActionApplier>::execute_action(runtime, context, &input.0).await
}

#[nirvash_runtime_contract(
    spec = Spec,
    binding = Binding,
    context = (),
    context_expr = (),
    probe_state = State,
    probe_output = Output,
    observe_state = observe_driver_state,
    output = observe_driver_output,
    fresh_runtime = Driver::default(),
    input = Input,
    input_codec = Input,
    dispatch_input = dispatch_input,
    tests(witness)
)]
impl Driver {
    #[nirvash_macros::contract_case(action = Action::Start)]
    async fn contract_start(&self) -> Output {
        let mut state = self.state.lock().expect("lock state");
        *state = State::Busy;
        Output::Ack
    }

    #[nirvash_macros::contract_case(action = Action::Stop)]
    async fn contract_stop(&self) -> Output {
        let mut state = self.state.lock().expect("lock state");
        *state = State::Idle;
        Output::Ack
    }
}

code_witness_test_main!();
