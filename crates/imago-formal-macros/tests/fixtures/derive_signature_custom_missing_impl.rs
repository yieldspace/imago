use imago_formal_core::Signature as _;
use imago_formal_macros::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Signature)]
#[signature(custom)]
struct State {
    ready: bool,
}

fn main() {
    let _ = State::bounded_domain();
}
