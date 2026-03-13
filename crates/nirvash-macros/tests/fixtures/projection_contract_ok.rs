use nirvash::BoolExpr;
use nirvash_lower::{FrontendSpec, ModelInstance, TemporalSpec};
use nirvash_conformance::ProtocolConformanceSpec;
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, nirvash_projection_contract};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
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

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;
    
    fn frontend_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

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

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        vec![ModelInstance::default()]
    }
}

impl TemporalSpec for Spec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
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
