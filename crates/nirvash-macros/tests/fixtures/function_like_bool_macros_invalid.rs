use nirvash_macros::nirvash_expr;

#[derive(Clone, Debug)]
struct State {
    ready: bool,
}

fn main() {
    let _ = nirvash_expr!(bad(state) => state.ready ^ state.ready);
}
