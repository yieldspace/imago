use nirvash::BoolExpr;
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, invariant, nirvash_expr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum OtherState {
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Tick,
}

struct Spec;

impl nirvash_lower::FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn frontend_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

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

#[invariant(Spec)]
fn wrong_state_type() -> BoolExpr<OtherState> {
    nirvash_expr! { wrong_state_type(_state) => true }
}

fn main() {}
