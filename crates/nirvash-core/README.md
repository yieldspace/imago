# nirvash-core

`nirvash` は、Rust から時相論理ベースの仕様を書き、そのまま形式検証できるライブラリです。  
`nirvash-core` はその中核で、遷移主体の reachable graph 探索、LTL/TLA+ practical subset、fairness、counterexample trace の土台を提供します。

## What It Provides

- `Signature`: bounded helper 型に有限 domain と値 invariant を与える trait
- `RelAtom` / `RelSet<T>` / `Relation2<A, B>`: Alloy 風の unary / binary relation を bounded finite domain 上で扱う relational kernel
- `TransitionSystem` / `TemporalSpec`: `initial_states + actions + transition_program()` を正本にした状態遷移と時相仕様の記述
- `Ltl`: `[]`, `<>`, `X`, `U`, `ENABLED`, `~>` を含む Rust DSL
- `ModelChecker`: reachable graph ベースの explicit/symbolic hybrid model checking
- `ActionApplier` / `StateObserver`: 実コード conformance の低レベル capability trait
- `StatePredicate` / `StepPredicate` / constraints / symmetry / fairness
- `pred!` / `step!` / `ltl!` と、`invariant!` / `property!` / `fairness!` などの記号寄り `macro_rules!` DSL
- `bounded_vec_domain` / `into_bounded_domain`: bounds-driven な domain 生成 helper

## Minimal Example

```rust
use nirvash_core::{ModelChecker, TransitionSystem};
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

    fn transition_program(&self) -> Option<nirvash_core::TransitionProgram<Self::State, Self::Action>> {
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

`nirvash-core` は runtime / checker / DSL を提供し、`nirvash-macros` は `#[invariant(...)]`、`#[state_constraint(Spec, cases("..."))]`、`#[action_constraint(Spec, cases("..."))]`、`#[subsystem_spec]`、`#[formal_tests(...)]` などの宣言を自動化します。bang macro 形式の `nirvash_core::invariant!` や `nirvash_core::property!` は互換用 wrapper で、通常は proc macro を正本として使います。`imagod-spec` はその上で `imagod` 全体の仕様を記述する利用例です。

`Signature` 系は通常どおり `#[derive(Signature)]`、`#[derive(ActionVocabulary)]`、`#[derive(RelAtom)]`、`#[derive(RelationalState)]` を使います。formal 用の doc/registry 向け cfg は derive macro 側が吸収するので、利用側で個別の cfg を書く必要はありません。

`Signature` derive の推奨順は次です。

- まず helper enum/newtype や bounded collection に使う
- 次に `#[signature(bounds(...))]` と `#[signature(filter(self => ...))]`、必要なら `#[signature_invariant(self => ...)]` で bounded domain を絞る
- それでも足りない型だけ `#[signature(custom)]` で companion trait を手書きする

重要なのは、`Signature` は **spec state space の正本ではない** ことです。  
通常 spec の source of truth は `TransitionSystem::initial_states()`、`TransitionSystem::actions()`、`TransitionSystem::transition_program()` で、checker と docs の `State Graph` / `Sequence Diagram` / `Algorithm View` もそこから reachable graph を構築します。system-level の並行性も top-level action を atomic に保った interleaving で表し、`Signature` は helper 型の有限境界を与えるための補助に限定します。

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

## System Interleaving

system-level spec は top-level action を atomic に保ち、reachable graph 上で interleaving をそのまま列挙します。

- atomic action は `TransitionSystem::actions()` と `TransitionSystem::transition()` に直接書きます
- docs と counterexample は individual atomic edge をそのまま表示します
- subsystem default は symbolic、system default は explicit ですが、どちらも同じ atomic reachable graph semantics を共有します
- tractability は `ModelCase` の action constraint と checker 上限で絞ります

## Runtime Conformance

`nirvash` は spec 単体の model checking だけでなく、runtime 実装が spec と同じ振る舞いをするかも検証できます。conformance API の正本は `nirvash_core::conformance` です。

- runtime capability
  - `ActionApplier`
    - `execute_action(Context, Action) -> ProbeOutput`
  - `StateObserver`
    - `observe_state(Context) -> ProbeState`
- spec 側契約
  - `ProtocolConformanceSpec`
    - `summarize_state(...)`
    - `summarize_output(...)`
    - `expected_output(...)`
    - `abstract_state(...)`
    - `abstract_output(...)`
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

runtime 側が返すのは concrete runtime から直接読める `ProbeState` / `ProbeOutput` だけで、履歴由来の一時事実や model 向け正規化は spec 側の `summarize_*` に閉じ込めます。`StateObserver` trait の associated type 名は `SummaryState` のままですが、`ProtocolRuntimeBinding` ではこれを `ProtocolConformanceSpec::ProbeState` に束縛します。

projection spec の正本は `nirvash_macros::nirvash_projection_model!` です。`state_summary` / `output_summary` / `state_abstract` / `output_abstract` を宣言すると、`summarize_state` / `summarize_output` / `abstract_state` / `abstract_output` と law test が自動生成されます。`probe_state_domain = ...` / `summary_output_domain = ...` を渡した boundary では law test が bounded exhaustive に切り替わります。`#[nirvash_projection_contract]` は低レベル fallback としてだけ残ります。

`nirvash_macros::code_tests` はこの契約だけを使って reachable graph の prefix を実コードへ適用し、各 step の `before_probe -> summarize_state -> abstract_state` と `after_probe -> summarize_state -> abstract_state` を `transition` の next state と突き合わせます。output も `probe_output -> summarize_output -> abstract_output` で比較するので、runtime 側は trace replay や shadow state を持たずに済みます。実運用では spec crate に `ProtocolConformanceSpec` を置き、runtime crate の integration test に `ProtocolRuntimeBinding` と `#[code_tests(...)]` を置く構成が依存方向を最も保ちやすいです。

`nirvash_macros::code_witness_tests` は `ProtocolInputWitnessBinding` を追加で使い、reachable graph から semantic case を自動検出して witness 単位の strict test を custom harness (`code_witness_test_main!()`) で個別実行します。`model_cases` は formal 側の探索分割に残しつつ、runtime binding 側は concrete input witness だけを実装すれば十分です。`Input = Action` 以外の witness は `#[derive(ProtocolInputWitness)]` で `ProtocolInputWitnessCodec<Action>` を自動実装でき、`canonical_positive` / `positive_family` / `negative_family` / `witness_name(action, kind, index)` を既定生成します。runtime-mode `#[nirvash_runtime_contract(..., input = Input, input_codec = Input, dispatch_input = ...)]` はその family API を使って witness 群を自動列挙します。

## Symbolic Backend

`ModelChecker` は `ModelCheckConfig.backend` と spec default から explicit / symbolic を切り替えます。

- 現在の実装は finite `Signature` domain を持つ state を対象に、reachable graph を exact に解く v1 です
- symbolic backend は bundled `z3` を default dependency として使います
- backend は `z3` crate を使いますが、runtime conformance 自体は引き続き explicit replay のままです
- 現時点では `ReachableGraph` exploration を対象にしており、`BoundedLasso` は fail-closed で拒否します
- `subsystem_spec` は default で symbolic、`system_spec` は default で explicit です
- symbolic backend は `transition_program()` と AST-native predicate/LTL/fairness を直接読みます。`transition()` / `successors()` / `successors_constrained()` のみを実装した spec は explicit backend 専用です

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
    // `ProtocolRuntimeBinding` maps this slot to `Spec::ProbeState`.
    type SummaryState = SpecState;
    type Context = RuntimeContext;

    async fn observe_state(&self, _context: &Self::Context) -> Self::SummaryState {
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
    type ProbeState = SpecState;
    type ProbeOutput = RuntimeOutput;
    type SummaryState = SpecState;
    type SummaryOutput = RuntimeOutput;

    fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
        *probe
    }

    fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
        *probe
    }

    fn expected_output(
        &self,
        state: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        let _ = (state, action, next);
        RuntimeOutput::Ack
    }

    fn abstract_state(&self, summary: &Self::SummaryState) -> Self::State {
        *summary
    }

    fn abstract_output(&self, summary: &Self::SummaryOutput) -> Self::ExpectedOutput {
        *summary
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

これにより `#[formal_tests(...)]` が付いた spec では reachable graph から生成した Mermaid の `State Graph` / `Sequence Diagram` section と、process summary の `Algorithm View` section が、すべての spec では registered invariant / property / fairness / constraint / subsystem 一覧を含む Mermaid の `Meta Model` section が rustdoc 上に注入されます。`State Graph` は docs 専用の boundary-path reduction を通す reduced reachable graph を使い、直線的な通常経路は 1 本の edge に畳まれ、同じ始点/終点に向かう平行 edge も 1 本にまとめられます。`Sequence Diagram` と `Algorithm View` は full reachable graph を補助に使いますが、actor/process の正本は action presentation metadata です。`State Graph` の edge label だけは Mermaid `stateDiagram-v2` の制約に合わせて `:` や `->` を安全な見た目へ正規化します。Mermaid runtime は doc fragment に inline で埋め込まれるため、`cargo doc --open` でも `file://` 経由の local asset 読み込みに依存せず表示できます。`build.rs` は再帰ビルドを避けるために `NIRVASH_DOCGEN_SKIP` を尊重します。

## State Graph Rendering

- `State Graph` の node は Mermaid の丸ノードで描画されます。
- node label は full state 全体ではなく、`initial` か `from Sx` に続く「前状態から変化した行」だけを表示します。
- full state の `Debug` 出力は図の下の `<details>` に畳み込まれた `Full State Legend` に残るため、図自体は密度を抑えつつ詳細も追えます。
- edge label は Mermaid parser が壊れないよう `:` や `->` を安全な見た目へ正規化して出力するため、`Manager(LoadExistingConfig)` のような括弧付き action 名も `manager - Load config → ...` のような collapsed label も崩さず表示できます。
- `State Graph` は reduced reachable graph を使うため、collapsed path の詳細は別 section に退避されます。
- reduced graph の node 数が `50` を超える case では図を出さず、omit note だけを表示します。

## Sequence Diagram Rendering

- `Sequence Diagram` は actor/process metadata に基づく multi-actor case だけを対象にし、single-actor spec では section 自体を出しません。
- lane は `State` / `Spec` ではなく、metadata が明示した actor role だけを participant にします。
- 分岐は `alt` / `else`、並行 edge は `par` / `and`、cycle と back-edge は 1 周分だけ `loop` で明示して停止します。
- reconverge して既に展開済みの state に入る枝は `continue at Sx` note で止め、shared suffix を無限に重複展開しないようにします。
- action 文言は first-line rustdoc と manual presentation metadata が正本で、system spec では `Client` / `Manager` / `Runner` のような role-level actor を使います。
- 各 step の note には `Sx -> Sy` と state delta か target state の要約を載せ、deadlock / terminal state も note で明示します。

## Algorithm View

- `Algorithm View` は `process X:` / `while TRUE:` ベースの summary を `text` code block で表示します。
- multi-actor case では actor ごとの process block、single-actor case では `process Spec:` を 1 本だけ出します。
- action presentation metadata の `Do / Send / Receive / Wait / Emit` を `do ...`, `send ...`, `receive ...`, `wait ...`, `emit ...` に写し、reachable graph の branch/cycle だけを `either:` / `or:` / `continue at Sx` へ要約します。
- registered invariant / property / fairness / constraint / symmetry 関数は `Algorithm View` の末尾に一覧表示されます。
