use std::sync::Mutex;

use nirvash::ActionVocabulary;
use nirvash_lower::FrontendSpec;
use nirvash_conformance::ProtocolConformanceSpec;
use nirvash_macros::{
    ActionVocabulary as FormalActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    nirvash_runtime_contract,
};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, FormalFiniteModelDomain)]
enum State {
    #[default]
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalActionVocabulary)]
enum Action {
    Start,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Output {
    #[default]
    Rejected,
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

    fn transition(&self, _state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
        Some(State::Idle)
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
        Output::Rejected
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

#[nirvash_runtime_contract(
    spec = Spec,
    binding = Binding,
    context = (),
    context_expr = (),
    summary = State,
    output = Output,
    summary_field = state,
    initial_summary = State::Idle,
    fresh_runtime = Driver::default(),
    tests(grouped)
)]
impl Driver {
    #[nirvash_macros::contract_case(action = Action::Start)]
    async fn contract_start(&self) {}
}

fn main() {}
