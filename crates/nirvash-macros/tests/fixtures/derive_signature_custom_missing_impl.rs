use nirvash_lower::FiniteModelDomain as _;
use nirvash_macros::FiniteModelDomain;

#[derive(Clone, Debug, PartialEq, Eq, FiniteModelDomain)]
#[finite_model_domain(custom)]
struct State {
    ready: bool,
}

fn main() {
    let _ = State::bounded_domain();
}
