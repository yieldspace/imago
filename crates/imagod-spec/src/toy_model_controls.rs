use nirvash_core::OpaqueModelValue;
use nirvash_core::TransitionSystem;
use nirvash_macros::{ActionVocabulary, Signature, formal_tests, subsystem_spec};

struct WorkerTag;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToyPhase {
    Idle,
    Busy,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToyState {
    worker: OpaqueModelValue<WorkerTag, 2>,
    phase: ToyPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Signature, ActionVocabulary)]
enum ToyAction {
    /// Start work
    Start,
    /// Finish work
    Finish,
    /// Block worker
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

    fn transition_state(&self, prev: &ToyState, action: &ToyAction) -> Option<ToyState> {
        let mut candidate = *prev;
        let allowed = match action {
            ToyAction::Start if matches!(prev.phase, ToyPhase::Idle) => {
                candidate.phase = ToyPhase::Busy;
                true
            }
            ToyAction::Finish if matches!(prev.phase, ToyPhase::Busy) => {
                candidate.phase = ToyPhase::Idle;
                true
            }
            _ => false,
        };
        allowed.then_some(candidate)
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

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.transition_state(state, action)
    }
}

#[formal_tests(spec = ToyModelControlSpec, cases = model_cases)]
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

    #[test]
    fn blocked_phase_remains_explicit_edge_case() {
        let blocked = ToyState {
            worker: OpaqueModelValue::new(0).expect("within bounds"),
            phase: ToyPhase::Blocked,
        };
        assert!(matches!(blocked.phase, ToyPhase::Blocked));
    }
}
