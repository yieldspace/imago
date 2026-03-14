use nirvash::{BoolExpr, StepExpr};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_lower::{ReductionClaim, SymmetryReduction};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, action_constraint, formal_tests, invariant, nirvash_expr,
    nirvash_step_expr, nirvash_transition_program, property, state_constraint, subsystem_spec,
    symmetry,
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

#[derive(Default)]
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

#[invariant(Spec)]
fn idle_is_valid() -> BoolExpr<State> {
    nirvash_expr! { idle_is_valid(_state) => true }
}

#[property(Spec)]
fn busy_leads_to_idle() -> nirvash::Ltl<State, Action> {
    nirvash::Ltl::leads_to(
        nirvash::Ltl::pred(nirvash_expr! { busy(state) => state.busy }),
        nirvash::Ltl::pred(nirvash_expr! { idle(state) => !state.busy }),
    )
}

#[state_constraint(Spec)]
fn allow_declared_states() -> BoolExpr<State> {
    nirvash_expr! { allow_declared_states(_state) => true }
}

#[action_constraint(Spec)]
fn allow_declared_edges() -> StepExpr<State, Action> {
    nirvash_step_expr! { allow_declared_edges(_prev, _action, _next) => true }
}

#[symmetry(Spec)]
fn identity_symmetry() -> ReductionClaim<SymmetryReduction<State>> {
    ReductionClaim::new(SymmetryReduction::new("identity", |state| *state))
}

fn spec_model_cases() -> Vec<ModelInstance<State, Action>> {
    vec![ModelInstance::default().with_check_deadlocks(false)]
}

fn spec_cases() -> Vec<Spec> {
    vec![Spec]
}

#[formal_tests(spec = Spec, cases = spec_cases)]
const _: () = ();

fn main() {
    let spec = Spec;
    assert!(nirvash_lower::TemporalSpec::invariants(&spec).len() == 1);
    assert!(nirvash_lower::TemporalSpec::properties(&spec).len() == 1);
}
