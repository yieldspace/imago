use nirvash_core::StatePredicate;
use nirvash_macros::{Signature as FormalSignature, invariant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum OtherState {
    Busy,
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

#[invariant(Spec)]
fn wrong_state_type() -> StatePredicate<OtherState> {
    StatePredicate::new("wrong_state_type", |_| true)
}

fn main() {}
