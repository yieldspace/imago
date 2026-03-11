use nirvash_core::{
    ModelCase, ModelCaseSource, BoolExpr, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{Signature as FormalSignature, nirvash_projection_contract};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Start,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Summary {
    state: State,
}

fn summarize_state(probe: &State) -> Summary {
    Summary { state: *probe }
}

fn summarize_output(probe: &bool) -> bool {
    *probe
}

fn abstract_state(_spec: &Spec, summary: &Summary) -> State {
    summary.state
}

fn abstract_output(_spec: &Spec, summary: &bool) -> bool {
    *summary
}

impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Idle, Action::Start) => Some(State::Busy),
            _ => None,
        }
    }
}

impl TemporalSpec for Spec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl ModelCaseSource for Spec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default()]
    }
}

#[nirvash_projection_contract(
    probe_state = State,
    probe_output = bool,
    summary_state = Summary,
    summary_output = bool,
    summarize_state = summarize_state,
    summarize_output = summarize_output,
    abstract_state = abstract_state,
    abstract_output = abstract_output
)]
impl ProtocolConformanceSpec for Spec {
    type ExpectedOutput = bool;

    fn expected_output(
        &self,
        _prev: &Self::State,
        _action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        next.is_some()
    }
}

fn main() {}
