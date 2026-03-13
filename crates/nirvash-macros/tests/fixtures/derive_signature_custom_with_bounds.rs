use nirvash_macros::FiniteModelDomain;

#[derive(Clone, Debug, PartialEq, Eq, FiniteModelDomain)]
#[finite_model_domain(custom, bounds(ready(domain = ready_domain)))]
struct State {
    ready: bool,
}

fn ready_domain() -> [bool; 2] {
    [false, true]
}

fn main() {}
