#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::{ProjectedState, SpecOracle};
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding,
    nirvash_project, nirvash_project_output,
};

#[derive(Clone, Copy, Debug, Default)]
#[code_tests]
struct Spec;

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    FormalFiniteModelDomain,
)]
enum State {
    Empty,
    Full,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    FormalFiniteModelDomain,
)]
enum Action {
    Fill,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Output {
    Ack,
}

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Empty]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Fill]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Empty, Action::Fill) => Some(State::Full),
            _ => None,
        }
    }
}

impl TemporalSpec for Spec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl SpecOracle for Spec {
    type ExpectedOutput = Output;

    fn expected_output(
        &self,
        _prev: &Self::State,
        _action: &Self::Action,
        _next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        Output::Ack
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Binding {
    filled: bool,
}

#[nirvash_binding(spec = Spec)]
impl Binding {
    #[nirvash(action = Action::Fill)]
    fn fill(&mut self) -> Output {
        self.filled = true;
        Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> ProjectedState<State> {
        if self.filled {
            ProjectedState::Partial(State::Full)
        } else {
            ProjectedState::Exact(State::Empty)
        }
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }
}

generated::install::unit_tests!(binding = Binding);

fn main() {}
