use nirvash::{
    BoundedDomain, ModelCase, ModelCaseSource, BoolExpr, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{Signature as FormalSignature, nirvash_projection_model};

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, FormalSignature)]
enum State {
    #[default]
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
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

#[derive(Clone, Debug, PartialEq, Eq, Default, FormalSignature)]
enum Effect {
    #[default]
    Ack,
    DropMe,
}

fn probe_state_domain() -> BoundedDomain<State> {
    <State as nirvash::Signature>::bounded_domain()
}

fn summary_output_domain() -> BoundedDomain<OutputSummary> {
    BoundedDomain::new(vec![
        OutputSummary::default(),
        OutputSummary {
            effects: vec![Effect::Ack],
        },
        OutputSummary {
            effects: vec![Effect::DropMe],
        },
    ])
}

impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

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
}

impl TemporalSpec for Spec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl ModelCaseSource for Spec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default()]
    }
}

nirvash_projection_model! {
    probe_state = State,
    probe_output = ProbeOutput,
    summary_state = Summary,
    summary_output = OutputSummary,
    abstract_state = State,
    expected_output = Vec<Effect>,
    probe_state_domain = probe_state_domain,
    summary_output_domain = summary_output_domain,
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
        effect @ Effect::Ack => effect.clone(),
        _effect @ Effect::DropMe => drop
    }
    impl ProtocolConformanceSpec for Spec {
        type ExpectedOutput = Vec<Effect>;

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            if next.is_some() {
                vec![Effect::Ack]
            } else {
                Vec::new()
            }
        }
    }
}

fn main() {}
