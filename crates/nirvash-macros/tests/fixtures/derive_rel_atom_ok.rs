use nirvash::{BoundedDomain, RelAtom, Signature};
use nirvash_macros::{RelAtom, Signature};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Signature, RelAtom)]
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
