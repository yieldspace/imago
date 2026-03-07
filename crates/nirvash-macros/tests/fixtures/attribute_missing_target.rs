use nirvash_core::StatePredicate;
use nirvash_macros::{Signature as FormalSignature, invariant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Tick,
}

struct Spec;

impl nirvash_core::TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn init(&self, _: &Self::State) -> bool {
        true
    }

    fn next(&self, _: &Self::State, _: &Self::Action, _: &Self::State) -> bool {
        false
    }
}

#[invariant]
fn missing_target() -> StatePredicate<State> {
    StatePredicate::new("missing_target", |_| true)
}

fn main() {}
