#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::{ProjectedState, SpecOracle};
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding, nirvash_project,
    nirvash_project_output,
};

#[derive(Clone, Copy, Debug, Default)]
#[code_tests]
struct Spec;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, FormalFiniteModelDomain,
)]
enum State {
    Idle,
    Fast,
    Slow,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, FormalFiniteModelDomain,
)]
enum Action {
    Start,
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Output {
    Ack,
    Rejected,
}

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start, Action::Reset]
    }

    fn transition_relation(&self, state: &Self::State, action: &Self::Action) -> Vec<Self::State> {
        match (state, action) {
            (State::Idle, Action::Start) => vec![State::Fast, State::Slow],
            (State::Fast | State::Slow, Action::Reset) => vec![State::Idle],
            _ => Vec::new(),
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
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        if next.is_some() {
            Output::Ack
        } else {
            Output::Rejected
        }
    }
}

#[derive(Clone, Debug)]
struct Binding {
    state: State,
}

impl Default for Binding {
    fn default() -> Self {
        Self { state: State::Idle }
    }
}

#[nirvash_binding(spec = Spec)]
impl Binding {
    #[nirvash(action = Action::Start)]
    fn start(&mut self) -> Output {
        match self.state {
            State::Idle => {
                self.state = State::Fast;
                Output::Ack
            }
            _ => Output::Rejected,
        }
    }

    #[nirvash(action = Action::Reset)]
    fn reset(&mut self) -> Output {
        match self.state {
            State::Fast | State::Slow => {
                self.state = State::Idle;
                Output::Ack
            }
            State::Idle => Output::Rejected,
        }
    }

    #[nirvash_project]
    fn project(&self) -> ProjectedState<State> {
        match self.state {
            State::Idle => ProjectedState::Exact(State::Idle),
            State::Fast => ProjectedState::Partial(State::Fast),
            State::Slow => ProjectedState::Partial(State::Slow),
        }
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }
}

generated::install::tests!(
    binding = Binding,
    profiles = [
        generated::profiles::smoke_default(),
        generated::profiles::unit_default().engines([
            nirvash_conformance::EnginePlan::ExplicitSuite,
            nirvash_conformance::EnginePlan::ProptestOnline {
                cases: 256,
                max_steps: 8,
            },
        ]),
    ],
);
