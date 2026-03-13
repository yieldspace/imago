use nirvash::BoolExpr;
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, state_constraint, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Tick,
}

struct Spec;

#[subsystem_spec(model_cases(spec_model_cases))]
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

    fn transition_program(&self) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(::nirvash::TransitionProgram::named("spec", vec![]))
    }
}

#[state_constraint(Spec, nope("case_a"))]
fn invalid_option() -> BoolExpr<State> {
    nirvash::BoolExpr::new("invalid_option", |_| true)
}

fn spec_model_cases() -> Vec<ModelInstance<State, Action>> {
    vec![ModelInstance::default()]
}

fn main() {}
