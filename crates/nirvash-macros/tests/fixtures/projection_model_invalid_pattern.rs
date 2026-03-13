use nirvash::BoolExpr;
use nirvash_lower::{FrontendSpec, ModelInstance, TemporalSpec};
use nirvash_conformance::ProtocolConformanceSpec;
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, nirvash_projection_model};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, FormalFiniteModelDomain)]
enum State {
    #[default]
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Start,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Summary {
    state: State,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct OutputSummary {
    effects: Vec<Effect>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct ProbeOutput {
    output: OutputSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum Effect {
    #[default]
    Ack,
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

    fn transition(&self, _state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
        Some(State::Idle)
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

nirvash_projection_model! {
    probe_state = State,
    probe_output = ProbeOutput,
    summary_state = Summary,
    summary_output = OutputSummary,
    abstract_state = State,
    expected_output = Vec<Effect>,
    state_seed = State::Idle,
    state_summary {
        state <= *probe
    }
    output_summary {
        effects <= probe.output.effects.clone()
    }
    state_abstract {
        state <= summary.state
    }
    output_abstract {
        effect => drop
    }
    impl ProtocolConformanceSpec for Spec {
        type ExpectedOutput = Vec<Effect>;

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            _next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            Vec::new()
        }
    }
}

fn main() {}
