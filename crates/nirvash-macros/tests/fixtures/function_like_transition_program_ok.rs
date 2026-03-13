use std::collections::BTreeMap;

use nirvash::{RelAtom, RelSet};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, nirvash_transition_program,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, FormalFiniteModelDomain)]
enum Item {
    Alpha,
    Beta,
}

impl RelAtom for Item {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Config {
    ready: bool,
    items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    ready: bool,
    count: i16,
    items: RelSet<Item>,
    queue: Vec<Item>,
    counters: BTreeMap<Item, usize>,
    config: Config,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Add(Item),
    Remove(Item),
}

fn program() -> nirvash::TransitionProgram<State, Action> {
    nirvash_transition_program! {
        rule activate when !prev.ready && matches!(action, Action::Add(_)) => {
            set ready <= true;
            set count <= if prev.ready { state.count * 2 } else { (state.count + 2) - 1 };
            insert items <= Item::Alpha;
            set items <= state.items.union(&prev.items);
            set config <= Config {
                ready: true,
                items: nirvash::sequence_update(state.config.items.clone(), 0, Item::Alpha),
                ..state.config.clone()
            };
            set queue <= nirvash::sequence_update(prev.queue.clone(), 1, Item::Beta);
            set counters <= nirvash::function_update(prev.counters.clone(), Item::Alpha, 2);
        }
        rule cleanup when prev.ready && matches!(action, Action::Remove(_)) => {
            remove items <= Item::Alpha;
        }
    }
}

fn main() {
    let program = program();
    let initial = State {
        ready: false,
        count: 0,
        items: RelSet::empty(),
        queue: vec![Item::Alpha, Item::Alpha],
        counters: BTreeMap::new(),
        config: Config {
            ready: false,
            items: vec![Item::Beta],
        },
    };
    let _ = program.evaluate(&initial, &Action::Add(Item::Alpha));
}
