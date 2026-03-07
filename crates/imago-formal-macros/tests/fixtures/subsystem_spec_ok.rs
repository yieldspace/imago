use imago_formal_core::{
    ActionConstraint, ModelCheckConfig, StateConstraint, StatePredicate, SymmetryReducer,
    TemporalSpec, TransitionSystem,
};
use imago_formal_macros::{
    Signature as FormalSignature, imago_action_constraint, imago_formal_tests, imago_invariant,
    imago_property, imago_state_constraint, imago_subsystem_spec, imago_symmetry,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Start,
    Stop,
}

#[derive(Default)]
struct Spec;

#[imago_subsystem_spec(
    invariants(idle_is_valid),
    illegal(),
    state_constraints(allow_declared_states),
    action_constraints(allow_declared_edges),
    properties(busy_leads_to_idle),
    fairness(),
    symmetry(identity_symmetry),
    checker_config(spec_checker_config)
)]
impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn init(&self, state: &Self::State) -> bool {
        matches!(state, State::Idle)
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        matches!(
            (prev, action, next),
            (State::Idle, Action::Start, State::Busy)
                | (State::Busy, Action::Stop, State::Idle)
        )
    }
}

#[imago_invariant]
fn idle_is_valid() -> StatePredicate<State> {
    StatePredicate::new("idle_is_valid", |_| true)
}

#[imago_property]
fn busy_leads_to_idle() -> imago_formal_core::Ltl<State, Action> {
    imago_formal_core::Ltl::leads_to(
        imago_formal_core::Ltl::pred(StatePredicate::new("busy", |state| matches!(state, State::Busy))),
        imago_formal_core::Ltl::pred(StatePredicate::new("idle", |state| matches!(state, State::Idle))),
    )
}

#[imago_state_constraint]
fn allow_declared_states() -> StateConstraint<State> {
    StateConstraint::new("allow_declared_states", |_| true)
}

#[imago_action_constraint]
fn allow_declared_edges() -> ActionConstraint<State, Action> {
    ActionConstraint::new("allow_declared_edges", |_, _, _| true)
}

#[imago_symmetry]
fn identity_symmetry() -> SymmetryReducer<State> {
    SymmetryReducer::new("identity", |state| *state)
}

fn spec_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        check_deadlocks: false,
        ..ModelCheckConfig::default()
    }
}

fn model_cases() -> Vec<Spec> {
    vec![Spec]
}

#[imago_formal_tests(spec = Spec, init = initial_state, cases = model_cases)]
const _: () = ();

impl Spec {
    fn initial_state(&self) -> State {
        State::Idle
    }
}

fn main() {
    let spec = Spec;
    assert!(spec.invariants().len() == 1);
    assert!(spec.properties().len() == 1);
}
