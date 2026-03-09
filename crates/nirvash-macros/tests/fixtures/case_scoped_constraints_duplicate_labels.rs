use nirvash_core::{ModelCase, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, action_constraint, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Tick,
}

struct Spec;

#[subsystem_spec(model_cases(spec_model_cases))]
impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Tick]
    }

    fn transition(&self, _: &Self::State, _: &Self::Action) -> Option<Self::State> {
        None
    }
}

#[action_constraint(Spec, cases("case_a", "case_a"))]
fn duplicate_case_labels() -> ActionConstraint<State, Action> {
    nirvash_core::ActionConstraint::new("duplicate_case_labels", |_, _, _| true)
}

fn spec_model_cases() -> Vec<ModelCase<State, Action>> {
    vec![ModelCase::new("case_a")]
}

fn main() {}
