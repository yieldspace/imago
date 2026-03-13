use nirvash::BoolExpr;
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, invariant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Tick,
}

struct Spec;

impl nirvash_lower::FrontendSpec for Spec {
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

#[invariant]
fn missing_target() -> BoolExpr<State> {
    BoolExpr::new("missing_target", |_| true)
}

fn main() {}
