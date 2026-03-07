# nirvash-core

`nirvash` は、Rust から時相論理ベースの仕様を書き、そのまま形式検証できるライブラリです。  
`nirvash-core` はその中核で、bounded domain、reachable graph 探索、LTL/TLA+ practical subset、fairness、counterexample trace、structural exhaustive test の土台を提供します。
`nirvash` は Rust から時相論理の仕様を書き、そのまま形式検証するためのライブラリです。

## What It Provides

- `Signature`: 有限な representative domain と値 invariant
- `TransitionSystem` / `TemporalSpec`: 状態遷移と時相仕様の記述
- `Ltl`: `[]`, `<>`, `X`, `U`, `ENABLED`, `~>` を含む Rust DSL
- `ModelChecker`: reachable graph ベースの model checking
- `StatePredicate` / `StepPredicate` / constraints / symmetry / fairness
- `pred!` / `step!` / `ltl!` と、`invariant!` / `property!` / `fairness!` などの記号寄り `macro_rules!` DSL

## Minimal Example

```rust
use nirvash_core::{ModelChecker, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, formal_tests, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Start,
    Finish,
}

#[derive(Default)]
struct Spec;

#[subsystem_spec]
impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn init(&self, state: &Self::State) -> bool {
        matches!(state, State::Idle)
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        matches!(
            (prev, action, next),
            (State::Idle, Action::Start, State::Busy)
                | (State::Busy, Action::Finish, State::Idle)
        )
    }
}

nirvash_core::invariant!(Spec, declared_states_are_valid(state) => {
    let _ = state;
    true
});

nirvash_core::property!(Spec, busy_leads_back_to_idle => leads_to(
    (pred!(busy(state) => matches!(state, State::Busy))),
    (pred!(idle(state) => matches!(state, State::Idle)))
));

#[formal_tests(spec = Spec, init = initial_state)]
const _: () = ();

impl Spec {
    fn initial_state(&self) -> State {
        State::Idle
    }
}

let spec = Spec::default();
let result = ModelChecker::new(&spec).check_all().expect("checker runs");
assert!(result.is_ok());
```

## Relationship To `nirvash-macros`

`nirvash-core` は runtime / checker / DSL を提供し、`nirvash-macros` は `#[invariant(...)]`、`#[subsystem_spec]`、`#[formal_tests(...)]` などの宣言を自動化します。bang macro 形式の `nirvash_core::invariant!` や `nirvash_core::property!` は内部でこれらの proc macro を使うため、利用 crate には `nirvash-core` と `nirvash-macros` の両方が必要です。`imagod-spec` はその上で `imagod` 全体の仕様を記述する利用例です。

## `cargo doc` Integration

`cargo doc` で spec の状態遷移図とメタモデル図を自動表示したい場合は、利用 crate に `build.rs` を追加して `nirvash_docgen::generate()` を呼びます。

```rust
fn main() {
    nirvash_docgen::generate().expect("failed to generate nirvash metamodel docs");
}
```

これにより `#[formal_tests(...)]` が付いた spec では reachable graph から生成した Mermaid の `State Graph` section が、すべての spec では registered invariant / property / fairness / constraint / subsystem 一覧を含む Mermaid の `Meta Model` section が rustdoc 上に注入されます。Mermaid runtime は local asset として `target/doc/static.files/` に配置されるため、生成物は CDN なしでそのまま表示できます。`build.rs` は再帰ビルドを避けるために `NIRVASH_DOCGEN_SKIP` を尊重します。
