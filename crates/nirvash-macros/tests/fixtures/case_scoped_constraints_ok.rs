use nirvash::{BoolExpr, StepExpr};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, action_constraint, nirvash_expr, nirvash_step_expr,
    nirvash_transition_program, state_constraint, subsystem_spec,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
struct State {
    busy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Start,
    Stop,
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
        vec![State { busy: false }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start, Action::Stop]
    }

    fn transition_program(&self) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start when matches!(action, Action::Start) && !prev.busy => {
                set busy <= true;
            }

            rule stop when matches!(action, Action::Stop) && prev.busy => {
                set busy <= false;
            }
        })
    }
}

#[state_constraint(Spec)]
fn global_state_constraint() -> BoolExpr<State> {
    nirvash_expr! { global_state_constraint(_state) => true }
}

#[state_constraint(Spec, cases("case_a"))]
fn only_case_a_state_constraint() -> BoolExpr<State> {
    nirvash_expr! { only_case_a_state_constraint(_state) => true }
}

#[action_constraint(Spec, cases("case_b"))]
fn only_case_b_action_constraint() -> StepExpr<State, Action> {
    nirvash_step_expr! { only_case_b_action_constraint(_prev, _action, _next) => true }
}

fn spec_model_cases() -> Vec<ModelInstance<State, Action>> {
    vec![ModelInstance::new("case_a"), ModelInstance::new("case_b")]
}

fn main() {
    let cases = Spec.model_instances();
    assert_eq!(cases[0].label(), "case_a");
    assert_eq!(cases[0].state_constraints().len(), 2);
    assert_eq!(cases[0].action_constraints().len(), 0);
    assert_eq!(cases[1].label(), "case_b");
    assert_eq!(cases[1].state_constraints().len(), 1);
    assert_eq!(cases[1].action_constraints().len(), 1);
}
