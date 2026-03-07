use nirvash_macros::{
    Signature as FormalSignature, imago_formal_tests, imago_invariant, imago_subsystem_spec,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Tick,
}

struct Spec;

#[imago_subsystem_spec]
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

#[imago_invariant(Spec)]
fn old_style_invariant() -> nirvash_core::StatePredicate<State> {
    nirvash_core::StatePredicate::new("old_style_invariant", |_| true)
}

#[imago_formal_tests(spec = Spec, init = initial_state)]
const _: () = ();

impl Spec {
    fn initial_state(&self) -> State {
        State::Idle
    }
}

fn main() {}
