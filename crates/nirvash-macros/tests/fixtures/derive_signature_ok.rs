use nirvash_core::{BoundedDomain, Signature};
use nirvash_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
enum Leaf {
    Idle,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
#[signature(custom)]
struct State {
    leaf: Leaf,
    ready: bool,
}

impl StateSignatureSpec for State {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            Self {
                leaf: Leaf::Idle,
                ready: false,
            },
            Self {
                leaf: Leaf::Busy,
                ready: true,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        !self.ready || matches!(self.leaf, Leaf::Busy)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Signature)]
#[signature(range = "0..=2")]
struct Counter(u8);

fn main() {
    let values = State::bounded_domain().into_vec();
    assert_eq!(values.len(), 2);
    assert!(values[1].invariant());
    assert_eq!(Counter::bounded_domain().into_vec(), vec![Counter(0), Counter(1), Counter(2)]);
}
