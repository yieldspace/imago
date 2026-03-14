#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::{GeneratedBinding, ProjectedState, SpecOracle};
use nirvash_lower::{FiniteModelDomain, FrontendSpec, TemporalSpec};
use nirvash_macros::{nirvash_binding, nirvash_project, nirvash_project_output};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum State {
    Idle,
    Busy,
}

impl FiniteModelDomain for State {
    fn finite_domain() -> nirvash::BoundedDomain<Self> {
        nirvash::BoundedDomain::new(vec![Self::Idle, Self::Busy])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Action {
    Tick,
}

impl FiniteModelDomain for Action {
    fn finite_domain() -> nirvash::BoundedDomain<Self> {
        nirvash::BoundedDomain::new(vec![Self::Tick])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Output {
    Ack,
}

#[derive(Default)]
struct Spec;

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

#[derive(Clone, Debug, PartialEq, Eq)]
struct DefaultFirstBinding {
    state: State,
}

impl Default for DefaultFirstBinding {
    fn default() -> Self {
        Self { state: State::Idle }
    }
}

#[nirvash_binding(spec = Spec)]
impl DefaultFirstBinding {
    fn new() -> Self {
        Self { state: State::Busy }
    }

    #[nirvash(action = Action::Tick)]
    fn tick(&mut self) -> Output {
        Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> ProjectedState<State> {
        ProjectedState::Exact(self.state)
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NewFirstBinding {
    state: State,
}

struct NewFirstBuilder;

impl NewFirstBuilder {
    fn build(self) -> NewFirstBinding {
        let _ = self;
        NewFirstBinding { state: State::Idle }
    }
}

#[nirvash_binding(spec = Spec)]
impl NewFirstBinding {
    fn new() -> Self {
        Self { state: State::Busy }
    }

    fn builder() -> NewFirstBuilder {
        NewFirstBuilder
    }

    #[nirvash(action = Action::Tick)]
    fn tick(&mut self) -> Output {
        Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> ProjectedState<State> {
        ProjectedState::Exact(self.state)
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }
}

#[test]
fn generated_fixture_prefers_default_over_new() {
    let fixture = <DefaultFirstBinding as GeneratedBinding<Spec>>::generated_fixture();
    let fixture = fixture
        .as_ref()
        .downcast_ref::<DefaultFirstBinding>()
        .expect("fixture downcast");

    assert_eq!(fixture.state, State::Idle);
}

#[test]
fn generated_fixture_prefers_new_over_builder() {
    let fixture = <NewFirstBinding as GeneratedBinding<Spec>>::generated_fixture();
    let fixture = fixture
        .as_ref()
        .downcast_ref::<NewFirstBinding>()
        .expect("fixture downcast");

    assert_eq!(fixture.state, State::Busy);
}
