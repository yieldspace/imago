use nirvash_core::{
    ActionConstraint, ModelCheckConfig, StateConstraint, StatePredicate, SymmetryReducer,
    TemporalSpec, TransitionSystem,
};
use nirvash_macros::{
    Signature as FormalSignature, action_constraint, formal_tests, invariant,
    property, state_constraint, subsystem_spec, symmetry,
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

#[subsystem_spec(checker_config(spec_checker_config))]
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

#[invariant(Spec)]
fn idle_is_valid() -> StatePredicate<State> {
    StatePredicate::new("idle_is_valid", |_| true)
}

#[property(Spec)]
fn busy_leads_to_idle() -> nirvash_core::Ltl<State, Action> {
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(StatePredicate::new("busy", |state| matches!(state, State::Busy))),
        nirvash_core::Ltl::pred(StatePredicate::new("idle", |state| matches!(state, State::Idle))),
    )
}

#[state_constraint(Spec)]
fn allow_declared_states() -> StateConstraint<State> {
    StateConstraint::new("allow_declared_states", |_| true)
}

#[action_constraint(Spec)]
fn allow_declared_edges() -> ActionConstraint<State, Action> {
    ActionConstraint::new("allow_declared_edges", |_, _, _| true)
}

#[symmetry(Spec)]
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

#[formal_tests(spec = Spec, init = initial_state, cases = model_cases)]
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
