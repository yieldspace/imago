# nirvash

`nirvash` は `imago` workspace の formal DSL / spec surface です。  
bounded domain、relation kernel、transition DSL、LTL/fairness、conformance trait、doc graph metadata を backend 非依存で提供します。

## Crate split

- `nirvash`
  - DSL / spec trait / conformance trait / doc graph shared type
- `nirvash-check`
  - `ModelChecker` front door と checker-facing API
- `nirvash-backends`
  - explicit / symbolic backend 実装

通常の runtime crate は `nirvash` だけを通常依存に取り、formal / doc / test 側だけが `nirvash-check` を使います。`z3` は `nirvash-backends` の通常依存として formal stack に常設されますが、`imagod` の通常依存木には入れません。

現状の symbolic backend は 2 系統です。

- `ModelBackend::Symbolic + ExplorationMode::ReachableGraph`
  - `TransitionProgram` と `SymbolicStateSpec` を使う relation-based safety path
- `ModelBackend::Symbolic + ExplorationMode::BoundedLasso`
  - 既存の candidate-graph bounded trace path

どちらも AST-native DSL を要求し、legacy closure path や未登録 helper / effect は fail-closed します。

## What It Provides

- `Signature`: bounded helper 型に有限 domain と値 invariant を与える trait
- `RelAtom` / `RelSet<T>` / `Relation2<A, B>`: relational kernel
- `TransitionSystem` / `TemporalSpec`: transition DSL と時相仕様の記述
- `Ltl`: `[]`, `<>`, `X`, `U`, `ENABLED`, `~>` を含む Rust DSL
- `ActionApplier` / `StateObserver`: runtime conformance capability
- `pred!` / `step!` / `ltl!` と registry macro

## Minimal Example

```rust
use nirvash::{TransitionProgram, TransitionSystem};
use nirvash_check::ModelChecker;
use nirvash_macros::{
    Signature as FormalSignature, formal_tests, nirvash_transition_program, subsystem_spec,
};

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

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start when matches!(action, Action::Start) && matches!(prev, State::Idle) => {
                set self <= State::Busy;
            }

            rule finish when matches!(action, Action::Finish) && matches!(prev, State::Busy) => {
                set self <= State::Idle;
            }
        })
    }
}

nirvash::invariant!(Spec, declared_states_are_valid(state) => {
    let _ = state;
    true
});

#[formal_tests(spec = Spec)]
const _: () = ();

let spec = Spec::default();
let result = ModelChecker::new(&spec).check_all().expect("checker runs");
assert!(result.is_ok());
```
