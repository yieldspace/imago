# nirvash-core

`nirvash` は、Rust から時相論理ベースの仕様を書き、そのまま形式検証できるライブラリです。  
`nirvash-core` はその中核で、bounded domain、reachable graph 探索、LTL/TLA+ practical subset、fairness、counterexample trace、structural exhaustive test の土台を提供します。

## What It Provides

- `Signature`: 有限な representative domain と値 invariant
- `TransitionSystem` / `TemporalSpec`: 状態遷移と時相仕様の記述
- `Ltl`: `[]`, `<>`, `X`, `U`, `ENABLED`, `~>` を含む Rust DSL
- `ModelChecker`: reachable graph ベースの model checking
- `ActionApplier` / `StateObserver`: 実コード conformance の低レベル capability trait
- `StatePredicate` / `StepPredicate` / constraints / symmetry / fairness
- `pred!` / `step!` / `ltl!` と、`invariant!` / `property!` / `fairness!` などの記号寄り `macro_rules!` DSL
- `bounded_vec_domain` / `into_bounded_domain`: bounds-driven な domain 生成 helper

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

`Signature` derive の推奨順は次です。

- まず field domain の直積に任せる
- 次に `#[signature(bounds(...))]` と `#[signature(filter(self => ...))]`、必要なら `#[signature_invariant(self => ...)]` で bounded domain を絞る
- それでも足りない型だけ `#[signature(custom)]` で companion trait を手書きする

field 単位では次を使えます。

- `#[sig(range = "0..=N")]`: scalar/newtype field の有限 range
- `#[sig(len = "A..=B")]`: `Vec<T>` field の bounded length
- `#[sig(domain = path)]`: `Vec<T>` / `[T; N]` / `BoundedDomain<T>` を返す関数で field domain を上書き
- manual fallback を短く書きたい場合は `nirvash_core::signature_spec!(StateSignatureSpec for State, representatives = ..., filter(self) => ..., invariant(self) => ...)` も使えます

## Runtime Conformance

`nirvash` は spec 単体の model checking だけでなく、runtime 実装が spec と同じ振る舞いをするかも検証できます。conformance API の正本は `nirvash_core::conformance` です。

- runtime capability
  - `ActionApplier`
    - `execute_action(Context, Action) -> Output`
  - `StateObserver`
    - `observe_state(Context) -> ObservedState`
- spec 側契約
  - `ProtocolConformanceSpec`
    - `expected_step(...)`
    - `project_state(...)`
    - `project_output(...)`
- spec と runtime の結合
  - `ProtocolRuntimeBinding`
    - `fresh_runtime(&spec)`
    - `context(&spec)`

`nirvash_macros::code_tests` はこの契約だけを使って reachable graph を replay し、runtime の observed state/output を spec 側の expected state/output に射影して比較します。runtime 側に spec 専用 field を追加する必要はありません。実運用では spec crate に `ProtocolConformanceSpec` を置き、runtime crate の integration test に `ProtocolRuntimeBinding` と `#[code_tests(...)]` を置く構成が依存方向を最も保ちやすいです。

```rust
use nirvash_core::conformance::{
    ActionApplier, ExpectedStep, ProtocolConformanceSpec, ProtocolRuntimeBinding, StateObserver,
};
use nirvash_core::TransitionSystem;
use nirvash_macros::{Signature as FormalSignature, code_tests, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum SpecState {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum RuntimeAction {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RuntimeContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeOutput {
    Ack,
}

#[derive(Default)]
struct Runtime(std::sync::Mutex<SpecState>);

impl ActionApplier for Runtime {
    type Action = RuntimeAction;
    type Output = RuntimeOutput;
    type Context = RuntimeContext;

    async fn execute_action(&self, _context: &Self::Context, action: &Self::Action) -> Self::Output {
        let mut state = self.0.lock().expect("runtime lock");
        *state = match (*state, *action) {
            (SpecState::Idle, RuntimeAction::Start) => SpecState::Busy,
            (SpecState::Busy, RuntimeAction::Stop) => SpecState::Idle,
            (current, _) => current,
        };
        RuntimeOutput::Ack
    }
}

impl StateObserver for Runtime {
    type ObservedState = SpecState;
    type Context = RuntimeContext;

    async fn observe_state(&self, _context: &Self::Context) -> Self::ObservedState {
        *self.0.lock().expect("runtime lock")
    }
}

#[derive(Default)]
struct Spec;

#[subsystem_spec]
impl TransitionSystem for Spec {
    type State = SpecState;
    type Action = RuntimeAction;

    fn init(&self, state: &Self::State) -> bool {
        matches!(state, SpecState::Idle)
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        matches!(
            (prev, action, next),
            (SpecState::Idle, RuntimeAction::Start, SpecState::Busy)
                | (SpecState::Busy, RuntimeAction::Stop, SpecState::Idle)
        )
    }
}

impl ProtocolConformanceSpec for Spec {
    type ExpectedOutput = RuntimeOutput;
    type ObservedState = SpecState;
    type ObservedOutput = RuntimeOutput;

    fn expected_step(
        &self,
        state: &Self::State,
        action: &Self::Action,
    ) -> Option<ExpectedStep<Self::State, Self::ExpectedOutput>> {
        match (state, action) {
            (SpecState::Idle, RuntimeAction::Start) => Some(ExpectedStep {
                next_state: SpecState::Busy,
                output: RuntimeOutput::Ack,
            }),
            (SpecState::Busy, RuntimeAction::Stop) => Some(ExpectedStep {
                next_state: SpecState::Idle,
                output: RuntimeOutput::Ack,
            }),
            _ => None,
        }
    }

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State {
        *observed
    }

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput {
        *observed
    }
}

struct Binding;

impl ProtocolRuntimeBinding<Spec> for Binding {
    type Runtime = Runtime;
    type Context = RuntimeContext;

    async fn fresh_runtime(_spec: &Spec) -> Self::Runtime {
        Runtime::default()
    }

    fn context(_spec: &Spec) -> Self::Context {
        RuntimeContext
    }
}

#[code_tests(spec = Spec, binding = Binding, init = initial_state)]
const _: () = ();

impl Spec {
    fn initial_state(&self) -> SpecState {
        SpecState::Idle
    }
}
```

## `cargo doc` Integration

`cargo doc` で spec の状態遷移図とメタモデル図を自動表示したい場合は、利用 crate に `build.rs` を追加して `nirvash_docgen::generate()` を呼びます。

```rust
fn main() {
    nirvash_docgen::generate().expect("failed to generate nirvash metamodel docs");
}
```

これにより `#[formal_tests(...)]` が付いた spec では reachable graph から生成した Mermaid の `State Graph` section が、すべての spec では registered invariant / property / fairness / constraint / subsystem 一覧を含む Mermaid の `Meta Model` section が rustdoc 上に注入されます。`State Graph` は docs 専用の boundary-path reduction を通すため、直線的な通常経路は 1 本の edge に畳まれ、同じ始点/終点に向かう平行 edge も 1 本にまとめられます。分岐/合流/終端/edge case state が優先的に残ります。Mermaid runtime は local asset として `target/doc/static.files/` に配置されるため、`cargo doc --open` でも CDN なしでそのまま表示できます。`build.rs` は再帰ビルドを避けるために `NIRVASH_DOCGEN_SKIP` を尊重します。

## State Graph Rendering

- `State Graph` の node は Mermaid の丸ノードで描画されます。
- node label は full state 全体ではなく、`initial` か `from Sx` に続く「前状態から変化した行」だけを表示します。
- full state の `Debug` 出力は図の下の `<details>` に畳み込まれた `Full State Legend` に残るため、図自体は密度を抑えつつ詳細も追えます。
- edge label は Mermaid parser が壊れないよう quoted label として出力されるため、`Manager(LoadExistingConfig)` のような括弧付き action 名もそのまま扱えます。
