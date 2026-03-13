use nirvash_macros::FiniteModelDomain;

#[derive(Clone, Debug, PartialEq, Eq, FiniteModelDomain)]
struct InvalidLen {
    #[finite_model(len = "0..=2")]
    ready: bool,
}

fn main() {}
