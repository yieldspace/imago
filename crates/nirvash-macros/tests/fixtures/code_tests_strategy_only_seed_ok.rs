#![allow(unused_imports)]

use nirvash::{BoolExpr, BoundedDomain};
use nirvash_conformance::SpecOracle;
use nirvash_lower::{FiniteModelDomain, FrontendSpec, TemporalSpec};
use nirvash_macros::{code_tests, nirvash_binding, nirvash_project, nirvash_project_output};

#[derive(Clone, Debug, Default)]
#[code_tests]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum State {
    Idle,
}

impl FiniteModelDomain for State {
    fn finite_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![Self::Idle])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Key(u32);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Action {
    Put(Key),
}

impl FiniteModelDomain for Action {
    fn finite_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![Self::Put(Key(0))])
    }
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
        vec![Action::Put(Key(0))]
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
    #[nirvash(action = Action::Put)]
    fn put(&mut self, _key: Key) -> Output {
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

fn main() {
    use generated::prelude::*;

    let _seed_profile = e2e_default();
    let _profile = generated::profiles::unit_default().with(nirvash_conformance::seeds! {
        strategy Key = proptest::sample::select(vec![Key(3), Key(5)]);
    });
}
