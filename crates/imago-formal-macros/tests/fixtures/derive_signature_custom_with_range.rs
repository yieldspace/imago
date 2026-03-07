use imago_formal_macros::Signature;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Signature)]
#[signature(custom, range = "0..=2")]
struct Counter(u8);

fn main() {}
