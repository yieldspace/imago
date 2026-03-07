use nirvash_core::Signature;
use nirvash_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
enum Leaf {
    Idle,
    Busy,
}

fn busy_only_domain() -> [Leaf; 1] {
    [Leaf::Busy]
}

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
#[signature(
    bounds(count(range = "0..=1"), entries(len = "0..=1")),
    filter(self => !self.entries.is_empty() || self.count == 0)
)]
#[signature_invariant(self => !self.entries.is_empty() || self.count == 0)]
struct AutoState {
    count: u8,
    entries: Vec<Leaf>,
    #[sig(domain = busy_only_domain)]
    leaf: Leaf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Signature)]
#[signature(range = "0..=2")]
struct Counter(u8);

fn main() {
    let values = AutoState::bounded_domain().into_vec();
    assert_eq!(values.len(), 5);
    assert!(values.iter().all(Signature::invariant));
    assert!(values.iter().all(|value| matches!(value.leaf, Leaf::Busy)));
    assert_eq!(
        Counter::bounded_domain().into_vec(),
        vec![Counter(0), Counter(1), Counter(2)]
    );
}
