use nirvash_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
#[signature(custom, bounds(ready(domain = ready_domain)))]
struct State {
    ready: bool,
}

fn ready_domain() -> [bool; 2] {
    [false, true]
}

fn main() {}
