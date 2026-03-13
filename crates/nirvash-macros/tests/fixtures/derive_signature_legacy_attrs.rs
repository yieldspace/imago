use nirvash_macros::FiniteModelDomain;

#[derive(Clone, Debug, PartialEq, Eq, FiniteModelDomain)]
#[signature(domain_fn = "State::representatives", invariant_fn = "State::signature_invariant")]
struct State {
    ready: bool,
}

fn main() {}
