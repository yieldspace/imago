use imago_formal_core::{StatePredicate, SymmetryReducer, TransitionSystem};
use imago_formal_macros::{
    Signature as FormalSignature, imago_invariant, imago_subsystem_spec, imago_symmetry,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Tick,
}

#[derive(Default)]
struct Spec;

#[imago_subsystem_spec(
    invariants(idle_is_valid),
    illegal(),
    properties(),
    fairness(),
    symmetry(identity_symmetry, duplicate_symmetry)
)]
impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn init(&self, state: &Self::State) -> bool {
        matches!(state, State::Idle)
    }

    fn next(&self, _: &Self::State, _: &Self::Action, _: &Self::State) -> bool {
        false
    }
}

#[imago_invariant]
fn idle_is_valid() -> StatePredicate<State> {
    StatePredicate::new("idle_is_valid", |_| true)
}

#[imago_symmetry]
fn identity_symmetry() -> SymmetryReducer<State> {
    SymmetryReducer::new("identity", |state| *state)
}

#[imago_symmetry]
fn duplicate_symmetry() -> SymmetryReducer<State> {
    SymmetryReducer::new("duplicate", |state| *state)
}

fn main() {}
