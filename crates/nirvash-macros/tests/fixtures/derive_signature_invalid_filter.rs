use nirvash_macros::FiniteModelDomain;

#[derive(Clone, Debug, PartialEq, Eq, FiniteModelDomain)]
#[finite_model_domain(filter(value => value.ready))]
struct InvalidFilter {
    ready: bool,
}

fn main() {}
