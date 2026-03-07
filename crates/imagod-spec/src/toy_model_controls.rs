use imago_formal_core::{
    ActionConstraint, Fairness, Ltl, OpaqueModelValue, Signature as _, StateConstraint,
    StatePredicate, StepPredicate, TransitionSystem,
};
use imago_formal_macros::{
    Signature as FormalSignature, imago_action_constraint, imago_fairness, imago_formal_tests,
    imago_invariant, imago_property, imago_state_constraint, imago_subsystem_spec,
};

struct WorkerTag;

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
enum ToyPhase {
    Idle,
    Busy,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
struct ToyState {
    worker: OpaqueModelValue<WorkerTag, 2>,
    phase: ToyPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
enum ToyAction {
    Start,
    Finish,
    Block,
}

#[derive(Debug, Clone, Copy)]
struct ToyModelControlSpec {
    initial_worker: OpaqueModelValue<WorkerTag, 2>,
}

impl Default for ToyModelControlSpec {
    fn default() -> Self {
        Self {
            initial_worker: OpaqueModelValue::new(0).expect("within bounds"),
        }
    }
}

impl ToyModelControlSpec {
    fn initial_state(&self) -> ToyState {
        ToyState {
            worker: self.initial_worker,
            phase: ToyPhase::Idle,
        }
    }

    fn model_cases() -> Vec<Self> {
        vec![
            Self {
                initial_worker: OpaqueModelValue::new(0).expect("within bounds"),
            },
            Self {
                initial_worker: OpaqueModelValue::new(1).expect("within bounds"),
            },
        ]
    }
}

#[imago_invariant]
fn blocked_states_remain_excluded() -> StatePredicate<ToyState> {
    StatePredicate::new("blocked_states_remain_excluded", |state| {
        !matches!(state.phase, ToyPhase::Blocked)
    })
}

#[imago_state_constraint]
fn exclude_blocked_states() -> StateConstraint<ToyState> {
    StateConstraint::new("exclude_blocked_states", |state| {
        !matches!(state.phase, ToyPhase::Blocked)
    })
}

#[imago_action_constraint]
fn disallow_block_transitions() -> ActionConstraint<ToyState, ToyAction> {
    ActionConstraint::new("disallow_block_transitions", |_, action, _| {
        !matches!(action, ToyAction::Block)
    })
}

#[imago_property]
fn busy_leads_back_to_idle() -> Ltl<ToyState, ToyAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("busy", |state| {
            matches!(state.phase, ToyPhase::Busy)
        })),
        Ltl::pred(StatePredicate::new("idle", |state| {
            matches!(state.phase, ToyPhase::Idle)
        })),
    )
}

#[imago_fairness]
fn finish_progress() -> Fairness<ToyState, ToyAction> {
    Fairness::weak(StepPredicate::new(
        "finish_progress",
        |prev, action, next| {
            matches!(prev.phase, ToyPhase::Busy)
                && matches!(action, ToyAction::Finish)
                && matches!(next.phase, ToyPhase::Idle)
        },
    ))
}

#[imago_subsystem_spec(
    invariants(blocked_states_remain_excluded),
    illegal(),
    state_constraints(exclude_blocked_states),
    action_constraints(disallow_block_transitions),
    properties(busy_leads_back_to_idle),
    fairness(finish_progress)
)]
impl TransitionSystem for ToyModelControlSpec {
    type State = ToyState;
    type Action = ToyAction;

    fn name(&self) -> &'static str {
        "toy_model_controls"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            ToyAction::Start if matches!(prev.phase, ToyPhase::Idle) => {
                candidate.phase = ToyPhase::Busy;
            }
            ToyAction::Finish if matches!(prev.phase, ToyPhase::Busy) => {
                candidate.phase = ToyPhase::Idle;
            }
            ToyAction::Block if matches!(prev.phase, ToyPhase::Busy) => {
                candidate.phase = ToyPhase::Blocked;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[imago_formal_tests(
    spec = ToyModelControlSpec,
    init = initial_state,
    cases = model_cases
)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cases_cover_all_declared_workers() {
        let workers = ToyModelControlSpec::model_cases()
            .into_iter()
            .map(|spec| spec.initial_worker.index())
            .collect::<Vec<_>>();
        assert_eq!(workers.len(), 2);
        assert!(workers.contains(&0));
        assert!(workers.contains(&1));
    }
}
