use std::collections::BTreeSet;

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

fn ready_or_idle() -> nirvash_core::BoolExpr<State> {
    nirvash_expr!(ready_or_idle(state) => state.ready || state.is_idle() || matches!(state.phase, Phase::Idle))
}

fn start_step() -> nirvash_core::StepExpr<State, Action> {
    nirvash_step_expr!(start_step(prev, action, next) => !prev.ready && action_is_start(action) && next.ready && prev.count < next.count)
}

fn program() -> nirvash_core::TransitionProgram<State, Action> {
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

#[test]
fn function_like_bool_macros_lower_to_ast() {
    let expr = ready_or_idle();
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
    assert!(step.is_ast_native());
    assert!(expr.eval(&prev));
    assert!(step.eval(&prev, &Action::Start, &next));
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
