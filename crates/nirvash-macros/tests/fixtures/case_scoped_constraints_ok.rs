use nirvash_core::{
    ActionConstraint, ModelCase, ModelCaseSource as _, StateConstraint, TransitionSystem,
};
use nirvash_macros::{Signature as FormalSignature, action_constraint, state_constraint, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Start,
    Stop,
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

#[state_constraint(Spec)]
fn global_state_constraint() -> StateConstraint<State> {
    StateConstraint::new("global_state_constraint", |_| true)
}

#[state_constraint(Spec, cases("case_a"))]
fn only_case_a_state_constraint() -> StateConstraint<State> {
    StateConstraint::new("only_case_a_state_constraint", |_| true)
}

#[action_constraint(Spec, cases("case_b"))]
fn only_case_b_action_constraint() -> ActionConstraint<State, Action> {
    ActionConstraint::new("only_case_b_action_constraint", |_, _, _| true)
}

fn spec_model_cases() -> Vec<ModelCase<State, Action>> {
    vec![ModelCase::new("case_a"), ModelCase::new("case_b")]
}

fn main() {
    let cases = Spec.model_cases();
    assert_eq!(cases[0].label(), "case_a");
    assert_eq!(cases[0].state_constraints().len(), 2);
    assert_eq!(cases[0].action_constraints().len(), 0);
    assert_eq!(cases[1].label(), "case_b");
    assert_eq!(cases[1].state_constraints().len(), 1);
    assert_eq!(cases[1].action_constraints().len(), 1);
}
