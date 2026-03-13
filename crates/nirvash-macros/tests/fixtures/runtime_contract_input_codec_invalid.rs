use nirvash_lower::FrontendSpec;
use nirvash_conformance::ProtocolConformanceSpec;
use nirvash_macros::nirvash_runtime_contract;

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum State {
    #[default]
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Start,
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
        vec![Action::Start]
    }

    fn transition(&self, _state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
        Some(State::Idle)
    }
}

impl ProtocolConformanceSpec for Spec {
    type ExpectedOutput = ();
    type ProbeState = State;
    type ProbeOutput = ();
    type SummaryState = State;
    type SummaryOutput = ();

    fn expected_output(
        &self,
        _prev: &Self::State,
        _action: &Self::Action,
        _next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {}

    fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
        *probe
    }

    fn summarize_output(&self, _probe: &Self::ProbeOutput) -> Self::SummaryOutput {}

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
        *summary
    }

    fn abstract_output(&self, _summary: &Self::SummaryOutput) -> Self::ExpectedOutput {}
}

#[derive(Debug, Default)]
struct Binding;

#[nirvash_runtime_contract(
    spec = Spec,
    binding = Binding,
    runtime = Binding,
    context = (),
    context_expr = (),
    input = Action,
    input_codec = Action,
    fresh_runtime = Binding::default(),
    tests(witness)
)]
impl Binding {}

fn main() {}
