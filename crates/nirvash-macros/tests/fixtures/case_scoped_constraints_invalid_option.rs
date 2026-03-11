use nirvash_core::{BoolExpr, ModelCase, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, state_constraint, subsystem_spec};

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

    fn transition_program(&self) -> Option<::nirvash_core::TransitionProgram<Self::State, Self::Action>> {
        Some(::nirvash_core::TransitionProgram::named("spec", vec![]))
    }
}

#[state_constraint(Spec, nope("case_a"))]
fn invalid_option() -> BoolExpr<State> {
    nirvash_core::BoolExpr::new("invalid_option", |_| true)
}

fn spec_model_cases() -> Vec<ModelCase<State, Action>> {
    vec![ModelCase::default()]
}

fn main() {}
