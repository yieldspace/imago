use nirvash_core::{
    ActionConstraint, Fairness, Ltl, OpaqueModelValue, Signature as _, StateConstraint,
    StatePredicate, StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    Signature as FormalSignature, action_constraint, fairness, formal_tests, invariant, property,
    state_constraint, subsystem_spec,
};

struct WorkerTag;

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
enum ToyPhase {
    Idle,
    Busy,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
struct ToyState {
    worker: OpaqueModelValue<WorkerTag, 2>,
    phase: ToyPhase,
}

impl ToyStateSignatureSpec for ToyState {
    fn representatives() -> nirvash_core::BoundedDomain<Self> {
        let workers = OpaqueModelValue::<WorkerTag, 2>::bounded_domain().into_vec();
        let mut states = Vec::with_capacity(workers.len() * 2);
        for worker in workers {
            states.push(Self {
                worker,
                phase: ToyPhase::Idle,
            });
            states.push(Self {
                worker,
                phase: ToyPhase::Busy,
            });
        }
        nirvash_core::BoundedDomain::new(states)
    }
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

#[invariant(ToyModelControlSpec)]
fn blocked_states_remain_excluded() -> StatePredicate<ToyState> {
    StatePredicate::new("blocked_states_remain_excluded", |state| {
        !matches!(state.phase, ToyPhase::Blocked)
    })
}

#[state_constraint(ToyModelControlSpec)]
fn exclude_blocked_states() -> StateConstraint<ToyState> {
    StateConstraint::new("exclude_blocked_states", |state| {
        !matches!(state.phase, ToyPhase::Blocked)
    })
}

#[action_constraint(ToyModelControlSpec)]
fn disallow_block_transitions() -> ActionConstraint<ToyState, ToyAction> {
    ActionConstraint::new("disallow_block_transitions", |_, action, _| {
        !matches!(action, ToyAction::Block)
    })
}

#[property(ToyModelControlSpec)]
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

#[fairness(ToyModelControlSpec)]
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

#[subsystem_spec]
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
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[formal_tests(
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
