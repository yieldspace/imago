# nirvash-core

`nirvash` は、Rust から時相論理ベースの仕様を書き、そのまま形式検証できるライブラリです。  
`nirvash-core` はその中核で、bounded domain、reachable graph 探索、LTL/TLA+ practical subset、fairness、counterexample trace、structural exhaustive test の土台を提供します。

## What It Provides

- `Signature`: 有限な representative domain と値 invariant
- `TransitionSystem` / `TemporalSpec`: 状態遷移と時相仕様の記述
- `Ltl`: `[]`, `<>`, `X`, `U`, `ENABLED`, `~>` を含む Rust DSL
- `ModelChecker`: reachable graph ベースの model checking
- `StatePredicate` / `StepPredicate` / constraints / symmetry / fairness

## Minimal Example

```rust
use nirvash_core::{Ltl, ModelChecker, StatePredicate, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, formal_tests, invariant, property, subsystem_spec};

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

#[invariant(Spec)]
fn declared_states_are_valid() -> StatePredicate<State> {
    StatePredicate::new("declared_states_are_valid", |_| true)
}

#[property(Spec)]
fn busy_leads_back_to_idle() -> Ltl<State, Action> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("busy", |state| matches!(state, State::Busy))),
        Ltl::pred(StatePredicate::new("idle", |state| matches!(state, State::Idle))),
    )
}

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

`nirvash-core` は runtime / checker / DSL を提供し、`nirvash-macros` は `#[invariant(...)]`、`#[subsystem_spec]`、`#[formal_tests(...)]` などの宣言を自動化します。`imagod-spec` はその上で `imagod` 全体の仕様を記述する利用例です。
