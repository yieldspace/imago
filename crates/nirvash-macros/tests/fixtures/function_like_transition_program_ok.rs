use nirvash::{BoundedDomain, RelAtom, RelSet, Signature};
use nirvash_macros::nirvash_transition_program;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Alpha,
    Beta,
}

impl Signature for Item {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![Self::Alpha, Self::Beta])
    }
}

impl RelAtom for Item {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    ready: bool,
    count: i16,
    items: RelSet<Item>,
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
    };
    let _ = program.evaluate(&initial, &Action::Add(Item::Alpha));
}
