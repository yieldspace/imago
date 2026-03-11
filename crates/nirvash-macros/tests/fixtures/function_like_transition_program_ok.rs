use std::collections::BTreeSet;

use nirvash_macros::nirvash_transition_program;

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    ready: bool,
    count: u8,
    items: BTreeSet<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Add(u8),
    Remove(u8),
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

fn main() {
    let program = program();
    let initial = State {
        ready: false,
        count: 0,
        items: BTreeSet::new(),
    };
    let _ = program.evaluate(&initial, &Action::Add(1));
}
