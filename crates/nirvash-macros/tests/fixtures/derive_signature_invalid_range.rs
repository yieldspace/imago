use nirvash_macros::FiniteModelDomain;

#[derive(Clone, Debug, PartialEq, Eq, FiniteModelDomain)]
#[finite_model_domain(range = "0..=3")]
struct InvalidRange {
    lhs: u8,
    rhs: u8,
}

fn main() {}
