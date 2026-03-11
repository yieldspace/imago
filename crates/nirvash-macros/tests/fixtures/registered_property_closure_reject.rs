use nirvash_core::TransitionSystem;
use nirvash_macros::{Signature as FormalSignature, property, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Tick,
}

struct Spec;

#[subsystem_spec]
impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Tick]
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash_core::TransitionProgram<Self::State, Self::Action>> {
        Some(::nirvash_core::TransitionProgram::named("spec", vec![]))
    }
}

#[property(Spec)]
fn bad_property() -> ::nirvash_core::Ltl<State, Action> {
    ::nirvash_core::Ltl::pred(::nirvash_core::BoolExpr::new("bad_property", |_| true))
}

fn main() {}
