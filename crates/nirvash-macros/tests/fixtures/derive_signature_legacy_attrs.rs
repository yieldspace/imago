use nirvash_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
#[signature(domain_fn = "State::representatives", invariant_fn = "State::signature_invariant")]
struct State {
    ready: bool,
}

fn main() {}
