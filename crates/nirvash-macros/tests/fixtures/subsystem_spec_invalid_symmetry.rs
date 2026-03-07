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

    fn init(&self, _: &Self::State) -> bool {
        true
    }

    fn next(&self, _: &Self::State, _: &Self::Action, _: &Self::State) -> bool {
        false
    }
}

fn main() {}
