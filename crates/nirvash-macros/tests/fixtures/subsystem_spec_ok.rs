use nirvash_core::{
    ActionConstraint, ModelCase, StateConstraint, StatePredicate, SymmetryReducer, TemporalSpec,
    TransitionSystem,
};
use nirvash_macros::{
    Signature as FormalSignature, action_constraint, formal_tests, invariant, property,
    state_constraint, subsystem_spec, symmetry,
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

#[subsystem_spec(model_cases(spec_model_cases))]
impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start, Action::Stop]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Idle, Action::Start) => Some(State::Busy),
            (State::Busy, Action::Stop) => Some(State::Idle),
            _ => None,
        }
    }
}

#[invariant(Spec)]
fn idle_is_valid() -> StatePredicate<State> {
    StatePredicate::new("idle_is_valid", |_| true)
}

#[property(Spec)]
fn busy_leads_to_idle() -> nirvash_core::Ltl<State, Action> {
    nirvash_core::Ltl::leads_to(
        nirvash_core::Ltl::pred(StatePredicate::new("busy", |state| {
            matches!(state, State::Busy)
        })),
        nirvash_core::Ltl::pred(StatePredicate::new("idle", |state| {
            matches!(state, State::Idle)
        })),
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

fn spec_model_cases() -> Vec<ModelCase<State, Action>> {
    vec![ModelCase::default().with_check_deadlocks(false)]
}

fn spec_cases() -> Vec<Spec> {
    vec![Spec]
}

#[formal_tests(spec = Spec, cases = spec_cases)]
const _: () = ();

fn main() {
    let spec = Spec;
    assert!(spec.invariants().len() == 1);
    assert!(spec.properties().len() == 1);
}
