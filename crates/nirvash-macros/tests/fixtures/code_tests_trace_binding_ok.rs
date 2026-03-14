#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::{ProjectedState, SpecOracle, TraceSink};
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding,
    nirvash_project, nirvash_project_output, nirvash_trace,
};
use serde_json::Value;

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
    Ready,
    Done,
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
    Run,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Output {
    Ack,
}

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Ready]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Run]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Ready, Action::Run) => Some(State::Done),
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
    done: bool,
}

#[nirvash_binding(spec = Spec)]
impl Binding {
    #[nirvash(action = Action::Run)]
    fn run(&mut self) -> Output {
        self.done = true;
        Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> ProjectedState<State> {
        if self.done {
            ProjectedState::Partial(State::Done)
        } else {
            ProjectedState::Exact(State::Ready)
        }
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }

    #[nirvash_trace]
    fn trace(&self, _output: &Output, sink: &mut dyn TraceSink<Spec>) {
        sink.record_update("done", Value::Bool(self.done));
    }
}

generated::install::trace_tests!(binding = Binding);

fn main() {}
