use nirvash_macros::{nirvash_expr, nirvash_step_expr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    ready: bool,
    count: u8,
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
    Stop,
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

fn main() {
    let expr = ready_or_idle();
    let step = start_step();
    let prev = State {
        ready: false,
        count: 0,
        phase: Phase::Idle,
    };
    let next = State {
        ready: true,
        count: 1,
        phase: Phase::Busy,
    };

    let _ = expr.eval(&prev);
    let _ = step.eval(&prev, &Action::Start, &next);
    assert!(expr.is_ast_native());
    assert!(step.is_ast_native());
}
