#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::SpecOracle;
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
    Idle,
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
    Tick,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Output {
    Ack,
}

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Tick]
    }

    fn transition(&self, state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
        Some(*state)
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
struct Binding;

#[nirvash_binding(spec = Spec)]
impl Binding {
    #[nirvash(action = Action::Tick)]
    fn tick(&mut self) -> Output {
        Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> State {
        State::Idle
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }
}

generated::install::kani_harnesses!(binding = Binding);

fn main() {}
