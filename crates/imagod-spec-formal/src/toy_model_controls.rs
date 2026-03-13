use nirvash::OpaqueModelValue;
use nirvash_lower::FrontendSpec;
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    SymbolicEncoding as FormalSymbolicEncoding, action_constraint, fairness, formal_tests,
    invariant, nirvash_expr, nirvash_step_expr, nirvash_transition_program, property,
    state_constraint, subsystem_spec,
};

struct WorkerTag;

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum ToyPhase {
    Idle,
    Busy,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
struct ToyState {
    worker: OpaqueModelValue<WorkerTag, 2>,
    phase: ToyPhase,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    ActionVocabulary,
)]
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

    #[allow(dead_code)]
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

#[invariant(ToyModelControlSpec)]
fn blocked_states_remain_excluded() -> nirvash::BoolExpr<ToyState> {
    nirvash_expr! { blocked_states_remain_excluded(state) =>
        !matches!(state.phase, ToyPhase::Blocked)
    }
}

#[state_constraint(ToyModelControlSpec)]
fn exclude_blocked_states() -> nirvash::BoolExpr<ToyState> {
    nirvash_expr! { exclude_blocked_states(state) =>
        !matches!(state.phase, ToyPhase::Blocked)
    }
}

#[action_constraint(ToyModelControlSpec)]
fn disallow_block_transitions() -> nirvash::StepExpr<ToyState, ToyAction> {
    nirvash_step_expr! { disallow_block_transitions(_prev, action, _next) =>
        !matches!(action, ToyAction::Block)
    }
}

#[property(ToyModelControlSpec)]
fn busy_leads_back_to_idle() -> nirvash::Ltl<ToyState, ToyAction> {
    nirvash::Ltl::leads_to(
        nirvash::Ltl::pred(nirvash_expr! { busy(state) =>
            matches!(state.phase, ToyPhase::Busy)
        }),
        nirvash::Ltl::pred(nirvash_expr! { idle(state) =>
            matches!(state.phase, ToyPhase::Idle)
        }),
    )
}

#[fairness(ToyModelControlSpec)]
fn finish_progress() -> nirvash::Fairness<ToyState, ToyAction> {
    nirvash::Fairness::weak(nirvash_step_expr! { finish_progress(prev, action, next) =>
        matches!(prev.phase, ToyPhase::Busy)
            && matches!(action, ToyAction::Finish)
            && matches!(next.phase, ToyPhase::Idle)
    })
}

#[subsystem_spec]
impl FrontendSpec for ToyModelControlSpec {
    type State = ToyState;
    type Action = ToyAction;

    fn frontend_name(&self) -> &'static str {
        "toy_model_controls"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start when matches!(action, ToyAction::Start)
                && matches!(prev.phase, ToyPhase::Idle) => {
                set phase <= ToyPhase::Busy;
            }

            rule finish when matches!(action, ToyAction::Finish)
                && matches!(prev.phase, ToyPhase::Busy) => {
                set phase <= ToyPhase::Idle;
            }
        })
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
