use nirvash::StepExpr;
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, action_constraint, subsystem_spec};

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

#[action_constraint(Spec, cases("case_a", "case_a"))]
fn duplicate_case_labels() -> StepExpr<State, Action> {
    nirvash::StepExpr::new("duplicate_case_labels", |_, _, _| true)
}

fn spec_model_cases() -> Vec<ModelInstance<State, Action>> {
    vec![ModelInstance::new("case_a")]
}

fn main() {}
