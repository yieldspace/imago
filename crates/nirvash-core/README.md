# nirvash-core

`nirvash` は、Rust から時相論理ベースの仕様を書き、そのまま形式検証できるライブラリです。  
`nirvash-core` はその中核で、遷移主体の reachable graph 探索、LTL/TLA+ practical subset、fairness、counterexample trace の土台を提供します。

## What It Provides

- `Signature`: bounded helper 型に有限 domain と値 invariant を与える trait
- `RelAtom` / `RelSet<T>` / `Relation2<A, B>`: Alloy 風の unary / binary relation を bounded finite domain 上で扱う relational kernel
- `TransitionSystem` / `TemporalSpec`: `initial_states + actions + transition` を正本にした状態遷移と時相仕様の記述
- `ConcurrentAction` / `ConcurrentTransitionSystem`: footprint 宣言から独立 atomic action の並行 step を自動合成する helper
- `Ltl`: `[]`, `<>`, `X`, `U`, `ENABLED`, `~>` を含む Rust DSL
- `ModelChecker`: reachable graph ベースの model checking
- `ActionApplier` / `StateObserver`: 実コード conformance の低レベル capability trait
- `StatePredicate` / `StepPredicate` / constraints / symmetry / fairness
- `pred!` / `step!` / `ltl!` と、`invariant!` / `property!` / `fairness!` などの記号寄り `macro_rules!` DSL
- `bounded_vec_domain` / `into_bounded_domain`: bounds-driven な domain 生成 helper

## Minimal Example

```rust
use nirvash_core::{ModelChecker, TransitionSystem};
use nirvash_macros::{formal_tests, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        match state {
            State::Idle => vec![(Action::Start, State::Busy)],
            State::Busy => vec![(Action::Finish, State::Idle)],
        }
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

#[formal_tests(spec = Spec)]
const _: () = ();

let spec = Spec::default();
let result = ModelChecker::new(&spec).check_all().expect("checker runs");
assert!(result.is_ok());
```

## Relationship To `nirvash-macros`

`nirvash-core` は runtime / checker / DSL を提供し、`nirvash-macros` は `#[invariant(...)]`、`#[subsystem_spec]`、`#[formal_tests(...)]` などの宣言を自動化します。bang macro 形式の `nirvash_core::invariant!` や `nirvash_core::property!` は内部でこれらの proc macro を使うため、利用 crate には `nirvash-core` と `nirvash-macros` の両方が必要です。`imagod-spec` はその上で `imagod` 全体の仕様を記述する利用例です。

`Signature` 系は通常どおり `#[derive(Signature)]`、`#[derive(ActionVocabulary)]`、`#[derive(RelAtom)]`、`#[derive(RelationalState)]` を使います。formal 用の doc/registry 向け cfg は derive macro 側が吸収するので、利用側で個別の cfg を書く必要はありません。

`Signature` derive の推奨順は次です。

- まず helper enum/newtype や bounded collection に使う
- 次に `#[signature(bounds(...))]` と `#[signature(filter(self => ...))]`、必要なら `#[signature_invariant(self => ...)]` で bounded domain を絞る
- それでも足りない型だけ `#[signature(custom)]` で companion trait を手書きする

重要なのは、`Signature` は **spec state space の正本ではない** ことです。  
通常 spec の source of truth は `TransitionSystem::initial_states()`、`TransitionSystem::actions()`、`TransitionSystem::transition()` で、checker も docs の State Graph もそこから reachable graph を構築します。並行 spec では `ConcurrentTransitionSystem::{initial_states, atomic_actions, atomic_transition, footprint_reads, footprint_writes}` を atomic 正本にし、top-level `TransitionSystem` 側で `ConcurrentAction` を合成します。`Signature` は helper 型の有限境界を与えるための補助に限定します。

field 単位では次を使えます。

- `#[sig(range = "0..=N")]`: scalar/newtype field の有限 range
- `#[sig(len = "A..=B")]`: `Vec<T>` field の bounded length
- `#[sig(domain = path)]`: `Vec<T>` / `[T; N]` / `BoundedDomain<T>` を返す関数で field domain を上書き
- manual fallback を短く書きたい場合は `nirvash_core::signature_spec!(StateSignatureSpec for State, representatives = ..., filter(self) => ..., invariant(self) => ...)` も使えます

## Relational Modeling

`nirvash-core` は relation-first な構造モデル記述も持ちます。v1 の対象は unary / binary relation です。

- atom は `#[derive(Signature, RelAtom)]` で有限 domain と stable index を与えます
- set relation は `RelSet<T>`、binary relation は `Relation2<A, B>` を使います
- 演算は `union` / `intersection` / `difference` / `subset_of` / `domain` / `range` / `transpose` / `join` / `cardinality` / `some` / `no` / `one` / `lone` を持ちます
- `transitive_closure()` は `Relation2<T, T>` だけを許し、異種 relation には `transitive_closure_checked()` で fail-closed にします
- state に relation field を持たせる場合は `#[derive(RelationalState)]` を付けると doc graph / rustdoc fragment が relation schema と Alloy 風 notation を表示します

## Declarative Concurrency

relation-first spec で service ごとの独立 transition をまとめたい場合は、`ConcurrentTransitionSystem` を使います。

- atomic action は `atomic_actions()` と `atomic_transition()` にだけ書きます
- read/write footprint は `footprint_reads()` / `footprint_writes()` で宣言します
- checker は non-empty 独立 subset を `ConcurrentAction<A>` として自動合成します
- doc graph の edge label は composite step を `parallel(a, b, c)` 形式で表示します
- tractability は `ModelCase` の action constraint で `ConcurrentAction::atoms()` / `arity()` を絞って与えます

## Runtime Conformance

`nirvash` は spec 単体の model checking だけでなく、runtime 実装が spec と同じ振る舞いをするかも検証できます。conformance API の正本は `nirvash_core::conformance` です。

- runtime capability
  - `ActionApplier`
    - `execute_action(Context, Action) -> Output`
  - `StateObserver`
    - `observe_state(Context) -> ObservedState`
- spec 側契約
  - `ProtocolConformanceSpec`
    - `expected_output(...)`
    - `project_state(...)`
    - `project_output(...)`
- spec と runtime の結合
  - `ProtocolRuntimeBinding`
    - `fresh_runtime(&spec)`
    - `context(&spec)`
  - `ProtocolInputWitnessBinding`
    - `fresh_session(&spec)`
    - `positive_witnesses(...)`
    - `negative_witnesses(...)`
    - `execute_input(...)`
    - `probe_context(...)`

`nirvash_macros::code_tests` はこの契約だけを使って reachable graph を replay し、runtime の observed state/output を spec 側の expected state/output に射影して比較します。runtime 側に spec 専用 field を追加する必要はありません。実運用では spec crate に `ProtocolConformanceSpec` を置き、runtime crate の integration test に `ProtocolRuntimeBinding` と `#[code_tests(...)]` を置く構成が依存方向を最も保ちやすいです。

`nirvash_macros::code_witness_tests` は `ProtocolInputWitnessBinding` を追加で使い、reachable graph から semantic case を自動検出して witness 単位の strict test を custom harness (`code_witness_test_main!()`) で個別実行します。`model_cases` は formal 側の探索分割に残しつつ、runtime binding 側は concrete input witness だけを実装すれば十分です。

```rust
use nirvash_core::conformance::{
    ActionApplier, ProtocolConformanceSpec, ProtocolRuntimeBinding, StateObserver,
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

    fn initial_states(&self) -> Vec<Self::State> {
        vec![SpecState::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![RuntimeAction::Start, RuntimeAction::Stop]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (SpecState::Idle, RuntimeAction::Start) => Some(SpecState::Busy),
            (SpecState::Busy, RuntimeAction::Stop) => Some(SpecState::Idle),
            _ => None,
        }
    }
}

impl ProtocolConformanceSpec for Spec {
    type ExpectedOutput = RuntimeOutput;
    type ObservedState = SpecState;
    type ObservedOutput = RuntimeOutput;

    fn expected_output(
        &self,
        state: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        let _ = (state, action, next);
        RuntimeOutput::Ack
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

#[code_tests(spec = Spec, binding = Binding)]
const _: () = ();
```

grouped な回帰だけで十分なら `#[code_tests(...)]` を使い、どの semantic case / witness が壊れたかを `cargo test -- --list` と個別再実行で追いたい場合は `#[code_witness_tests(...)]` と `code_witness_test_main!()` を使います。

## `cargo doc` Integration

`cargo doc` で spec の状態遷移図とメタモデル図を自動表示したい場合は、利用 crate に `build.rs` を追加して `nirvash_docgen::generate()` を呼びます。

```rust
fn main() {
    nirvash_docgen::generate().expect("failed to generate nirvash metamodel docs");
}
```

これにより `#[formal_tests(...)]` が付いた spec では reachable graph から生成した Mermaid の `State Graph` section が、すべての spec では registered invariant / property / fairness / constraint / subsystem 一覧を含む Mermaid の `Meta Model` section が rustdoc 上に注入されます。`State Graph` は docs 専用の boundary-path reduction を通すため、直線的な通常経路は 1 本の edge に畳まれ、同じ始点/終点に向かう平行 edge も 1 本にまとめられます。分岐/合流/終端/edge case state が優先的に残ります。Mermaid runtime は doc fragment に inline で埋め込まれるため、`cargo doc --open` でも `file://` 経由の local asset 読み込みに依存せず表示できます。`build.rs` は再帰ビルドを避けるために `NIRVASH_DOCGEN_SKIP` を尊重します。

## State Graph Rendering

- `State Graph` の node は Mermaid の丸ノードで描画されます。
- node label は full state 全体ではなく、`initial` か `from Sx` に続く「前状態から変化した行」だけを表示します。
- full state の `Debug` 出力は図の下の `<details>` に畳み込まれた `Full State Legend` に残るため、図自体は密度を抑えつつ詳細も追えます。
- edge label は Mermaid parser が壊れないよう quoted label として出力されるため、`Manager(LoadExistingConfig)` のような括弧付き action 名もそのまま扱えます。
