use std::collections::BTreeSet;

use nirvash_macros::nirvash_transition_program;

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    items: BTreeSet<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Add(u8),
}

fn main() {
    let _ = nirvash_transition_program! {
        rule add when matches!(action, Action::Add(_)) => {
            effect items <= 1;
        }
    };
}
