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

現状の backend semantics は次のとおりです。

- `ModelBackend::Explicit + ExplorationMode::ReachableGraph`
  - `ExplicitModelCheckOptions::current()` に対応する exact in-memory BFS reachable graph
- `ModelBackend::Explicit + ExplorationMode::BoundedLasso`
  - `ExplicitModelCheckOptions::current()` に対応する explicit bounded prefix / lasso enumeration
- `ModelBackend::Symbolic + ExplorationMode::ReachableGraph`
  - `TransitionProgram` と `SymbolicStateSpec` / `SymbolicSortSpec` を使い、transition relation を solver で解く successor enumeration safety path
- `ModelBackend::Symbolic + ExplorationMode::BoundedLasso`
  - `SymbolicModelCheckOptions::current()` に対応する direct SMT bounded-lasso unrolling + loop selector

symbolic backend は AST-native DSL を要求し、legacy closure path や未登録 helper / effect は fail-closed します。schema validation は direct field read だけでなく pure call の receiver / argument read path、property、fairness にも掛かり、state schema には sort metadata も保持されます。
explicit backend は exact state equality ベースの symmetry canonicalization を使い、temporal property / fairness と併用できます。

`ModelCheckConfig` は共通 knob に加えて backend-specific option を持ちます。

- `explicit: ExplicitModelCheckOptions`
  - 現時点では `state_storage = InMemoryExact`、`reachability = BreadthFirst`、`bounded_lasso = EnumeratedPaths`
  - `simulation = ExplicitSimulationOptions { runs: 1, max_depth: 32, seed: 0 }` で `ModelChecker::simulate()` の deterministic random walk を設定
- `symbolic: SymbolicModelCheckOptions`
  - 現時点では `successors = SolverEnumeration`、`bounded_lasso = DirectSmt`

これらは current implementation を present tense で表す public contract で、Milestone 11/12 の explicit scaling / advanced symbolic work はこの surface に拡張していきます。

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
