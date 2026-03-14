#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::{SpecOracle, TraceSink};
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding,
    nirvash_project, nirvash_project_output, nirvash_trace,
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default)]
#[code_tests(
    models = [small, boundary, e2e_default],
    profiles = [
        smoke_default = {
            coverage = [transitions],
            engines = [explicit_suite],
        },
        unit_default = {
            coverage = [transitions, transition_pairs(2), guard_boundaries],
            engines = [explicit_suite, proptest_online(cases = 64, steps = 4)],
        },
        e2e_default = {
            coverage = [property_prefixes],
            engines = [trace_validation],
        },
    ],
)]
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
    Busy,
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
    Start,
    Stop,
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
        vec![Action::Start, Action::Stop]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Idle, Action::Start) => Some(State::Busy),
            (State::Busy, Action::Stop) => Some(State::Idle),
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
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        if next.is_some() {
            Output::Ack
        } else {
            Output::Rejected
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
                self.state = State::Busy;
                Output::Ack
            }
            _ => Output::Rejected,
        }
    }

    #[nirvash(action = Action::Stop)]
    fn stop(&mut self) -> Output {
        match self.state {
            State::Busy => {
                self.state = State::Idle;
                Output::Ack
            }
            _ => Output::Rejected,
        }
    }

    #[nirvash_project]
    fn project(&self) -> State {
        self.state
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }

    #[nirvash_trace]
    fn trace(&self, _output: &Output, sink: &mut dyn TraceSink<Spec>) {
        sink.record_update("state", Value::String(format!("{:?}", self.state)));
    }
}

nirvash::import_generated_tests!(spec = Spec, binding = Binding);

fn main() {}
