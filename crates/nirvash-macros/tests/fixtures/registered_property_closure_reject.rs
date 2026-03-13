use nirvash_lower::FrontendSpec;
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, property, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Tick,
}

struct Spec;

#[subsystem_spec]
impl FrontendSpec for Spec {
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

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(::nirvash::TransitionProgram::named("spec", vec![]))
    }
}

#[property(Spec)]
fn bad_property() -> ::nirvash::Ltl<State, Action> {
    ::nirvash::Ltl::pred(::nirvash::BoolExpr::new("bad_property", |_| true))
}

fn main() {}
