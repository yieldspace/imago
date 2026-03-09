use nirvash_core::{Relation2, RelSet};
use nirvash_macros::{RelAtom, RelationalState, Signature};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Signature, RelAtom)]
enum Atom {
    Root,
    Dependency,
}

#[derive(Debug, RelationalState)]
struct DemoState {
    requires: Relation2<Atom, Atom>,
    allowed: RelSet<Atom>,
    counter: u8,
}

fn main() {
    let _ = DemoState {
        requires: Relation2::from_pairs([(Atom::Root, Atom::Dependency)]),
        allowed: RelSet::from_items([Atom::Root]),
        counter: 1,
    };
}
