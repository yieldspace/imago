use std::collections::BTreeSet;

use nirvash::{BoolExprAst, UpdateAst, UpdateOp, UpdateValueExprAst};
use nirvash_macros::{nirvash_expr, nirvash_step_expr, nirvash_transition_program};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    ready: bool,
    count: u8,
    items: BTreeSet<u8>,
    phase: Phase,
}

impl State {
    fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Start,
    Add(u8),
    Remove(u8),
}

fn action_is_start(action: &Action) -> bool {
    matches!(action, Action::Start)
}

nirvash::register_symbolic_pure_helpers!("target_phase");

fn target_phase(action: &Action) -> Option<Phase> {
    match action {
        Action::Start => Some(Phase::Busy),
        _ => None,
    }
}

fn missing_target_phase(action: &Action) -> Option<Phase> {
    target_phase(action)
}

fn ready_or_idle() -> nirvash::BoolExpr<State> {
    nirvash_expr!(ready_or_idle(state) => state.ready || state.is_idle() || matches!(state.phase, Phase::Idle))
}

fn effective_count() -> nirvash::BoolExpr<State> {
    nirvash_expr!(effective_count(state) => (if state.ready { state.count } else { 0 }) >= 1)
}

fn start_step() -> nirvash::StepExpr<State, Action> {
    nirvash_step_expr!(start_step(prev, action, next) => !prev.ready && action_is_start(action) && next.ready && prev.count < next.count)
}

fn program() -> nirvash::TransitionProgram<State, Action> {
    nirvash_transition_program! {
        rule activate when !prev.ready && matches!(action, Action::Add(_)) => {
            set ready <= true;
            set count <= state.count + 1;
            insert items <= 1;
        }
        rule cleanup when prev.ready && matches!(action, Action::Remove(_)) => {
            remove items <= 1;
        }
    }
}

fn helper_wrapped_program() -> nirvash::TransitionProgram<State, Action> {
    nirvash_transition_program! {
        rule activate when target_phase(action).is_some() => {
            set phase <= target_phase(action).expect("activate guard matched");
        }
    }
}

fn missing_helper_wrapped_program() -> nirvash::TransitionProgram<State, Action> {
    nirvash_transition_program! {
        rule activate when missing_target_phase(action).is_some() => {
            set phase <= missing_target_phase(action).expect("activate guard matched");
        }
    }
}

fn pure_call_path_program() -> nirvash::TransitionProgram<State, Action> {
    nirvash_transition_program! {
        rule activate when prev.ready.clone() && target_phase(action).is_some() => {
            set phase <= prev.phase.clone();
        }
    }
}

#[test]
fn function_like_bool_macros_lower_to_ast() {
    let expr = ready_or_idle();
    let effective = effective_count();
    let step = start_step();

    let prev = State {
        ready: false,
        count: 0,
        items: BTreeSet::new(),
        phase: Phase::Idle,
    };
    let next = State {
        ready: true,
        count: 1,
        items: BTreeSet::new(),
        phase: Phase::Busy,
    };

    assert!(expr.is_ast_native());
    assert!(effective.is_ast_native());
    assert!(step.is_ast_native());
    assert!(expr.eval(&prev));
    assert!(effective.eval(&next));
    assert!(step.eval(&prev, &Action::Start, &next));
    assert!(matches!(expr.ast(), Some(BoolExprAst::Or(_))));
}

#[test]
fn transition_program_macro_builds_ast_rules() {
    let program = program();

    let initial = State {
        ready: false,
        count: 0,
        items: BTreeSet::from([2]),
        phase: Phase::Idle,
    };
    let next = program
        .evaluate(&initial, &Action::Add(7))
        .expect("rule evaluation")
        .expect("matching rule");
    assert_eq!(next.ready, true);
    assert_eq!(next.count, 1);
    assert_eq!(next.items, BTreeSet::from([1, 2]));
    assert!(program.rules()[0].is_ast_native());
    assert!(program.rules()[0].guard_ast().is_some());
    assert!(program.rules()[0].update_ast().is_some());
    match program.rules()[0].update_ast().expect("update ast") {
        UpdateAst::Sequence(ops) => {
            assert!(matches!(
                &ops[0],
                UpdateOp::Assign {
                    value_ast: UpdateValueExprAst::Literal { .. },
                    ..
                }
            ));
            assert!(matches!(
                &ops[1],
                UpdateOp::Assign {
                    value_ast: UpdateValueExprAst::Add { .. },
                    ..
                }
            ));
            assert!(matches!(
                &ops[2],
                UpdateOp::SetInsert {
                    item_ast: UpdateValueExprAst::Literal { .. },
                    ..
                }
            ));
        }
        UpdateAst::Choice(_) => panic!("fixture program should stay deterministic"),
    }

    let cleanup_state = State {
        ready: true,
        count: 1,
        items: BTreeSet::from([1, 2]),
        phase: Phase::Busy,
    };
    let cleaned = program
        .evaluate(&cleanup_state, &Action::Remove(1))
        .expect("cleanup evaluation")
        .expect("matching cleanup rule");
    assert_eq!(cleaned.items, BTreeSet::from([2]));
}

#[test]
fn transition_program_macro_tracks_wrapped_helper_registrations() {
    let registered = helper_wrapped_program();
    let missing = missing_helper_wrapped_program();

    assert_eq!(registered.first_unencodable_symbolic_node(), None);
    assert_eq!(
        missing.first_unencodable_symbolic_node(),
        Some("missing_target_phase")
    );
}

#[test]
fn transition_program_macro_tracks_pure_call_read_paths() {
    let program = pure_call_path_program();

    assert_eq!(program.symbolic_state_paths(), vec!["phase", "ready"]);
}
