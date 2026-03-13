use nirvash_macros::FiniteModelDomain;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FiniteModelDomain)]
#[finite_model_domain(custom, range = "0..=2")]
struct Counter(u8);

fn main() {}
