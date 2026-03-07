use nirvash_core::{OpaqueModelValue, Signature as _, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, formal_tests, subsystem_spec};

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
}

fn model_cases() -> Vec<ToyModelControlSpec> {
    vec![
        ToyModelControlSpec {
            initial_worker: OpaqueModelValue::new(0).expect("within bounds"),
        },
        ToyModelControlSpec {
            initial_worker: OpaqueModelValue::new(1).expect("within bounds"),
        },
    ]
}

nirvash_core::invariant!(ToyModelControlSpec, blocked_states_remain_excluded(state) => {
    !matches!(state.phase, ToyPhase::Blocked)
});

nirvash_core::state_constraint!(ToyModelControlSpec, exclude_blocked_states(state) => {
    !matches!(state.phase, ToyPhase::Blocked)
});

nirvash_core::action_constraint!(
    ToyModelControlSpec,
    disallow_block_transitions(prev, action, next) => {
        let _ = (prev, next);
        !matches!(action, ToyAction::Block)
    }
);

nirvash_core::property!(ToyModelControlSpec, busy_leads_back_to_idle => leads_to(
    (pred!(busy(state) => matches!(state.phase, ToyPhase::Busy))),
    (pred!(idle(state) => matches!(state.phase, ToyPhase::Idle)))
));

nirvash_core::fairness!(weak ToyModelControlSpec, finish_progress(prev, action, next) => {
    matches!(prev.phase, ToyPhase::Busy)
        && matches!(action, ToyAction::Finish)
        && matches!(next.phase, ToyPhase::Idle)
});

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
        let workers = model_cases()
            .into_iter()
            .map(|spec| spec.initial_worker.index())
            .collect::<Vec<_>>();
        assert_eq!(workers.len(), 2);
        assert!(workers.contains(&0));
        assert!(workers.contains(&1));
    }
}
