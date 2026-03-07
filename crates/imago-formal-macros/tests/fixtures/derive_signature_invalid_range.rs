use imago_formal_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
#[signature(range = "0..=3")]
struct InvalidRange {
    lhs: u8,
    rhs: u8,
}

fn main() {}
