use nirvash_macros::{Signature as FormalSignature, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Tick,
}

struct Spec;

#[subsystem_spec(invariants(legacy_invariant))]
impl nirvash_core::TransitionSystem for Spec {
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

fn main() {}
