#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::SpecOracle;
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding,
    nirvash_fixture, nirvash_project, nirvash_project_output,
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
    Seeded,
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
    Read,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Output {
    Ack,
}

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Seeded]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Read]
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
struct Binding {
    value: u8,
}

#[nirvash_binding(spec = Spec)]
impl Binding {
    #[nirvash(create)]
    fn create(fixture: u8) -> Self {
        Self { value: fixture }
    }

    #[nirvash_fixture]
    fn fixture() -> u8 {
        7
    }

    #[nirvash(action = Action::Read)]
    fn read(&mut self) -> Output {
        let _ = self.value;
        Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> State {
        let _ = self.value;
        State::Seeded
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }
}

fn main() {
    let _profile = generated::profiles::unit_default().with(nirvash_conformance::seeds! {
        fixture = 9_u8;
        initial_state = State::Seeded;
    });
}
