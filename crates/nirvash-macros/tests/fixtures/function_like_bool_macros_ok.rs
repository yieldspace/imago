use nirvash::{RelAtom, RelSet};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, nirvash_expr, nirvash_step_expr,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Item {
    Alpha,
    Beta,
}

impl RelAtom for Item {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    ready: bool,
    count: i8,
    phase: Phase,
    active: RelSet<Item>,
    pending: RelSet<Item>,
}

impl State {
    fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
}

fn action_is_start(action: &Action) -> bool {
    matches!(action, Action::Start)
}

fn ready_or_idle() -> nirvash::BoolExpr<State> {
    nirvash_expr!(ready_or_idle(state) => state.ready || state.is_idle() || matches!(state.phase, Phase::Idle))
}

fn count_window() -> nirvash::BoolExpr<State> {
    nirvash_expr!(count_window(state) => state.count <= 3 && state.count >= 1 && state.count > 0)
}

fn effective_count() -> nirvash::BoolExpr<State> {
    nirvash_expr!(effective_count(state) => (if state.ready { state.count } else { 0 }) >= 1)
}

fn transformed_count() -> nirvash::BoolExpr<State> {
    nirvash_expr!(transformed_count(state) => ((state.count * 2) - 1) >= (-state.count))
}

fn set_ops() -> nirvash::BoolExpr<State> {
    nirvash_expr!(set_ops(state) =>
        state.active.clone().union(&state.pending.clone()).contains(&Item::Beta)
            && state.pending
                .clone()
                .intersection(&state.active.clone())
                .difference(&state.active.clone().difference(&state.pending.clone()))
                .subset_of(&state.active.clone().union(&state.pending.clone()))
    )
}

fn start_step() -> nirvash::StepExpr<State, Action> {
    nirvash_step_expr!(start_step(prev, action, next) => !prev.ready && action_is_start(action) && next.ready && prev.count < next.count && next.count >= 1)
}

fn main() {
    let expr = ready_or_idle();
    let count_window = count_window();
    let effective_count = effective_count();
    let transformed_count = transformed_count();
    let set_ops = set_ops();
    let step = start_step();
    let prev = State {
        ready: false,
        count: 0,
        phase: Phase::Idle,
        active: RelSet::from_items([Item::Alpha]),
        pending: RelSet::empty(),
    };
    let next = State {
        ready: true,
        count: 1,
        phase: Phase::Busy,
        active: RelSet::from_items([Item::Alpha]),
        pending: RelSet::from_items([Item::Alpha, Item::Beta]),
    };

    let _ = expr.eval(&prev);
    let _ = count_window.eval(&next);
    let _ = effective_count.eval(&next);
    let _ = transformed_count.eval(&next);
    let _ = set_ops.eval(&next);
    let _ = step.eval(&prev, &Action::Start, &next);
    assert!(expr.is_ast_native());
    assert!(count_window.is_ast_native());
    assert!(effective_count.is_ast_native());
    assert!(transformed_count.is_ast_native());
    assert!(set_ops.is_ast_native());
    assert!(step.is_ast_native());
}
