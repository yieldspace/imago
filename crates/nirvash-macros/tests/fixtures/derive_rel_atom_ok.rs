use nirvash::{BoundedDomain, RelAtom};
use nirvash_lower::FiniteModelDomain;
use nirvash_macros::{FiniteModelDomain, RelAtom};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FiniteModelDomain, RelAtom)]
enum Atom {
    Root,
    Dependency,
}

fn main() {
    assert_eq!(Atom::bounded_domain().into_vec(), vec![Atom::Root, Atom::Dependency]);
    assert_eq!(Atom::Root.rel_index(), 0);
    assert_eq!(Atom::Dependency.rel_index(), 1);
    assert_eq!(Atom::rel_from_index(1), Some(Atom::Dependency));
    assert_eq!(Atom::rel_from_index(2), None);
    assert_eq!(Atom::Root.rel_label(), "Root");
    let _ = BoundedDomain::singleton(Atom::Root);
}
