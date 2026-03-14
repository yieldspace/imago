#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::{ProjectedState, SpecOracle, TraceSink};
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding, nirvash_project,
    nirvash_project_output, nirvash_trace,
};
use serde_json::json;

#[derive(Clone, Copy, Debug, Default)]
#[code_tests]
struct TraceSpec;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, FormalFiniteModelDomain,
)]
enum TraceState {
    Zero,
    One,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, FormalFiniteModelDomain,
)]
enum TraceAction {
    Tick,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum TraceOutput {
    Ack,
}

impl FrontendSpec for TraceSpec {
    type State = TraceState;
    type Action = TraceAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![TraceState::Zero]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![TraceAction::Tick]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (TraceState::Zero, TraceAction::Tick) => Some(TraceState::One),
            (TraceState::One, TraceAction::Tick) => Some(TraceState::One),
        }
    }
}

impl TemporalSpec for TraceSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl SpecOracle for TraceSpec {
    type ExpectedOutput = TraceOutput;

    fn expected_output(
        &self,
        _prev: &Self::State,
        _action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        let _ = next;
        TraceOutput::Ack
    }
}

#[derive(Clone, Debug, Default)]
struct TraceBinding {
    count: u8,
}

#[nirvash_binding(spec = TraceSpec)]
impl TraceBinding {
    #[nirvash(action = TraceAction::Tick)]
    fn tick(&mut self) -> TraceOutput {
        if self.count == 0 {
            self.count = 1;
        }
        TraceOutput::Ack
    }

    #[nirvash_project]
    fn project(&self) -> ProjectedState<TraceState> {
        if self.count == 0 {
            ProjectedState::Exact(TraceState::Zero)
        } else {
            ProjectedState::Exact(TraceState::One)
        }
    }

    #[nirvash_project_output]
    fn project_output(_action: &TraceAction, output: &TraceOutput) -> TraceOutput {
        output.clone()
    }

    #[nirvash_trace]
    fn trace(&self, _output: &TraceOutput, sink: &mut dyn TraceSink<TraceSpec>) {
        sink.record_update("count", json!(self.count));
    }
}

generated::install::trace_tests!(binding = TraceBinding);
