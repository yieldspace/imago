use nirvash_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
#[signature(filter(value => value.ready))]
struct InvalidFilter {
    ready: bool,
}

fn main() {}
