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

#[imago_invariant(Spec)]
fn old_style_invariant() -> nirvash_core::BoolExpr<State> {
    nirvash_core::BoolExpr::new("old_style_invariant", |_| true)
}

#[imago_formal_tests(spec = Spec)]
const _: () = ();

fn main() {}
