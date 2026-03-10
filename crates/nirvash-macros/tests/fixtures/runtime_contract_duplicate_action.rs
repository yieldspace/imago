use nirvash_core::{ActionVocabulary, TransitionSystem, conformance::ProtocolConformanceSpec};
use nirvash_macros::{
    ActionVocabulary as FormalActionVocabulary, Signature as FormalSignature,
    nirvash_runtime_contract,
};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
struct Summary {
    state: State,
}

impl Default for Summary {
    fn default() -> Self {
        Self { state: State::Idle }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature, FormalActionVocabulary)]
enum Action {
    Start,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Output {
    #[default]
    Rejected,
}

impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

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
    type ProbeState = Summary;
    type ProbeOutput = Output;
    type SummaryState = Summary;
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
        probe.clone()
    }

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
        summary.state
    }

    fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
        *summary
    }
}

#[derive(Debug, Default)]
struct Driver {
    summary: AsyncSummaryCell,
}

#[derive(Debug, Default)]
struct AsyncSummaryCell(std::sync::Mutex<Summary>);

impl AsyncSummaryCell {
    async fn lock(&self) -> std::sync::MutexGuard<'_, Summary> {
        self.0.lock().expect("lock summary")
    }
}

#[nirvash_runtime_contract(
    spec = Spec,
    binding = Binding,
    context = (),
    context_expr = (),
    summary = Summary,
    output = Output,
    summary_field = summary,
    initial_summary = Summary::default(),
    fresh_runtime = Driver::default(),
    tests(grouped)
)]
impl Driver {
    #[nirvash_macros::contract_case(action = Action::Start, update(state = State::Idle))]
    async fn one(&self) {}

    #[nirvash_macros::contract_case(action = Action::Start, update(state = State::Idle))]
    async fn two(&self) {}
}

fn main() {}
