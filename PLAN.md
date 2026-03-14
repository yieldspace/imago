# RFC 0002: `nirvash` における Spec-to-Code Verification と Refinement Checking の導入

## RFC TODO

- [x] RFC TODO 管理の骨格を導入し、実装順で進捗を追跡できるようにする
- [x] Phase 1A: `nirvash-conformance` に `RefinementMap` trait と `step_refines_relation` を追加する
- [x] Phase 1B: summary helper / `enabled_from_summary` を relation-aware に整理する
- [x] Phase 1C: code witness / macro harness を relation API に追従させる
- [x] Phase 2: `trace_refines` を explicit backend で実装する
- [x] Phase 3: `ExplicitModelChecker` / `SymbolicModelChecker` へ checker front door を分離する
- [x] Phase 4: sound reduction と heuristic reduction の API を分離する
- [x] Phase 5: Rust 検証アダプタ (`proptest-state-machine` / `Kani` / `loom`) を追加する
- [x] Phase 6: proof export を pretty / sound に分離する

- Status: Implemented (Phase 1-6 complete)
- Created: 2026-03-13
- Target crates: `nirvash`, `nirvash-ir`, `nirvash-lower`, `nirvash-check`, `nirvash-backends`, `nirvash-conformance`, `nirvash-proof`

## 0. 要約

本 RFC は、`nirvash` を **spec authoring / model checking の基盤**から、**spec と Rust 実装の整合性を検証する基盤**へ拡張する提案である。

現行の `nirvash` は、frontend DSL・core IR・lowering・checker front door・backend・conformance・proof export へ crate split されており、explicit reachable graph、direct SMT safety/temporal path、witness/runtime binding、`PrettySpecExporter` / `SoundProofExporter` を提供している。この再編は方向として正しい。

Phase 1-6 の実装で解消した主な課題は次のとおりである。

1. `LoweredSpec` は `SpecCore` を持つが、同時に checker-facing な executable/symbolic artifact を保持しており、checker 境界も `CheckerSpec` の operational API を公開している。
2. `TemporalSpec::fairness()` は依然として legacy fairness surface を返し、IR 側の `WF { view, action } / SF { view, action }` が authoring surface まで届いていない。
3. checker front door を `ExplicitModelChecker` / `SymbolicModelChecker` に分離し、symbolic-only path から `FiniteModelDomain` 要件を外した。
4. `ModelInstance::with_state_abstraction` / `with_por` を廃止し、`SoundReduction` / `HeuristicReduction` に置き換えた。
5. `nirvash-conformance` を deterministic equality 依存から外し、`step_refines_summary` / `step_refines_relation` / `trace_refines` を result-returning API として導入した。
6. `nirvash-proof` を `PrettySpecExporter` と `SoundProofExporter` に分離し、obligation export と可読 dump の責務を分けた。

本 RFC は、これらを踏まえて、**refinement mapping と trace validation を中心にした code verification layer** を導入する。具体的には、

- refinement map を first-class にする
- step equality ではなく relation-based refinement を導入する
- trace validation を constrained model checking として実装する
- explicit / symbolic checker front door を分離する
- sound reduction と heuristic reduction を型・API で分離する
- `proptest-state-machine`, `Kani`, `loom` を `nirvash-conformance` に接続する

という 6 点を提案する。

## 1. 背景

`nirvash` の README が示すとおり、現在の workspace は `nirvash` を formal frontend DSL、`nirvash-ir` を backend 非依存の `SpecCore`、`nirvash-lower` を lowering boundary、`nirvash-check` を checker front door、`nirvash-backends` を explicit/symbolic backend、`nirvash-conformance` を witness/runtime binding/refinement assert、`nirvash-proof` を SMT-LIB/TLA module export として分離している。また README は、explicit reachable graph を exact BFS、symbolic reachable graph を direct SMT safety path、symbolic bounded lasso を direct SMT temporal path と説明している。<sup>[1]</sup>

同時に、`nirvash-lower` では `LoweredSpec` が `core: SpecCore` を持ちながら、checker-facing な executable/symbolic artifact を並列に保持し、`TemporalSpec` は `Vec<Fairness<Self::State, Self::Action>>` を返し、`CheckerSpec` も `initial_states()`, `actions()`, `transition_program()` などの operational surface を露出している。<sup>[2]</sup>

現在の `nirvash-check` は `ExplicitModelChecker` と `SymbolicModelChecker` を公開し、symbolic-only path では `FiniteModelDomain` を要求しない。<sup>[3]</sup>

`nirvash-conformance` は `ActionApplier` / `StateObserver` に加えて `RefinementMap`, `step_refines_summary`, `step_refines_relation`, `trace_refines` を持ち、runtime 観測と abstract trace validation を result-returning witness API として提供する。<sup>[4]</sup>

`nirvash-proof` は `PrettySpecExporter` と `SoundProofExporter` を提供し、可読 dump と sound obligation export を fail-closed に分離している。<sup>[5]</sup>

一方で `nirvash-ir` 自体は、`FairnessDecl::WF { view, action } / SF { view, action }` を持っており、再編の方向は合っている。ただし IR には `Opaque(String)`、`Comprehension { domain: String, body: String }`、`Choice { domain: String, body: String }`、`Quantified { domain: String, body: String }` のような stringly node も残っている。<sup>[6]</sup>

したがって、いま必要なのは crate split の再設計ではなく、**spec から実コードへの検証経路を意味論的に整備すること**である。

## 2. 問題設定

### 2.1 `nirvash` は spec を書けるが、実装が spec を満たすことを十分には検証できない

TLA+ 系の古典的な立場では、下位仕様や実装が上位仕様を正しく実装していることは refinement mapping によって説明される。Abadi と Lamport は refinement mapping を「低レベル仕様の状態空間から高レベル仕様の状態空間への写像」と位置づけ、必要に応じて auxiliary variable を追加することで refinement mapping の存在を保証できることを示した。<sup>[7]</sup>

この立場に立つと、`nirvash` が次に持つべき第一級の対象は「モデル検査そのもの」ではなく、**implementation state → abstract spec state の写像**である。

### 2.2 実装と spec のズレは 1-step equality だけでは捉えられない

分散システムや非決定的 API では、実装が内部状態や補助状態を持つため、観測された 1 ステップが spec の 1 ステップと一対一に対応しないことが多い。TLA+ の trace validation に関する近年の研究では、実行トレースを高水準 TLA+ 仕様と照合する問題を constrained model checking に還元し、完全状態ではなく spec 変数の更新だけを記録した不完全トレースでも有効に検証できることが示されている。<sup>[8]</sup>

したがって、旧 `assert_step_refinement` のような deterministic equality ベースの API は v0 の smoke test としてはよいが、正式な code verification layer の中核にはならない。

### 2.3 Rust 実装の検証は単一のツールでは足りない

Rust 側には用途の異なる複数の検証手法がある。`proptest` の state machine testing は abstract reference state machine と SUT の差分を最小反例に縮約しやすい。`Kani` は proof harness を単位として bounded な網羅検査を行う bit-precise model checker である。`loom` は C11 memory model 下で concurrent execution の並び替えを系統的に探索する。<sup>[9]</sup>

よって `nirvash` 側の API は、これらを一つの refinement model に接続できる形で設計されるべきである。

## 3. 目標

本 RFC の目標は次のとおりである。

1. `nirvash` において **spec-to-code verification** を第一級機能にする。
2. deterministic / nondeterministic の両方の spec を扱えるようにする。
3. hidden step、stuttering、補助状態、線形化点を含む実装を扱えるようにする。
4. 逐次テスト、bounded exhaustive checking、schedule exploration、deductive verification を一貫した refinement API に接続する。
5. 既存の authoring surface (`pred!`, `step!`, `ltl!`, `TransitionProgram`) と crate split を大きく壊さない。

## 4. 非目標

本 RFC は以下を目標としない。

1. 任意の Rust crate を自動的に完全証明すること。
2. TLAPS 相当の proof manager を全面再実装すること。
3. refinement map を完全自動合成すること。
4. heuristics を完全に禁止すること。

## 5. 提案

## 5.1 refinement map を first-class にする

`nirvash-conformance` に、実装状態と抽象状態の対応を表す trait を導入する。

```rust
pub trait RefinementMap<Spec: FrontendSpec> {
    type ImplState;
    type ImplInput;
    type ImplOutput;
    type AuxState;

    fn abstract_state(&self, c: &Self::ImplState, aux: &Self::AuxState) -> Spec::State;

    fn candidate_actions(
        &self,
        before: &Self::ImplState,
        input: &Self::ImplInput,
        output: &Self::ImplOutput,
        after: &Self::ImplState,
        aux: &Self::AuxState,
    ) -> Vec<Spec::Action>;

    fn abstract_output(
        &self,
        _output: &Self::ImplOutput,
        _aux: &Self::AuxState,
    ) -> Option<SpecOutput> {
        None
    }

    fn init_aux(&self, _c: &Self::ImplState) -> Self::AuxState;

    fn update_aux(
        &self,
        before: &Self::ImplState,
        input: &Self::ImplInput,
        output: &Self::ImplOutput,
        after: &Self::ImplState,
        aux: &Self::AuxState,
    ) -> Self::AuxState;

    fn hidden_step(
        &self,
        _before: &Self::ImplState,
        _after: &Self::ImplState,
        _aux: &Self::AuxState,
    ) -> bool {
        false
    }
}
```

ここで `AuxState` は history / prophecy / ghost state 相当を表す。これにより、実装の内部キュー、pending request、linearization point、batching 状態を abstract state へ押し込まずに扱える。

### 5.1.1 理由

Abadi/Lamport の refinement mapping は、低レベル状態から高レベル状態への写像に auxiliary variable を組み合わせる考え方を与える。<sup>[7]</sup> また TLAPS は標準的な safety proof と step simulation を扱う。<sup>[10]</sup> したがって `nirvash` でも、counterexample witness より前に refinement map そのものを明示的に持つ方が自然である。

## 5.2 step refinement を equality ではなく relation にする

旧 `assert_step_refinement` は

```rust
expected_next = transition(before, action)
assert_eq!(projected_after, expected_next)
```

という deterministic equality ベースである。<sup>[4]</sup>

これを relation-based API に置き換える。

```rust
pub fn step_refines_relation<Spec, R>(
    spec: &Spec,
    map: &R,
    before: &R::ImplState,
    input: &R::ImplInput,
    output: &R::ImplOutput,
    after: &R::ImplState,
    aux: &R::AuxState,
) -> Result<StepRefinementWitness<Spec>, StepRefinementError>
where
    Spec: FrontendSpec,
    R: RefinementMap<Spec>;
```

意味論は次とする。

- `a ∈ candidate_actions(...)` のいずれかが選べること
- `r(before)` から `a` により許される abstract successor が存在すること
- `r(after)` がその successor の一つであること
- 必要なら hidden/stuttering step を有限回挟めること

これにより nondeterministic spec、retry、idempotent API、実装内の micro-step を吸収できる。

## 5.3 trace refinement を導入する

`nirvash-conformance` に次の API を導入する。

```rust
pub struct ObservedEvent<I, O, C> {
    pub before: C,
    pub input: I,
    pub output: O,
    pub after: C,
}

pub struct TraceRefinementConfig {
    pub allow_stuttering: bool,
    pub max_hidden_steps_per_event: usize,
    pub require_initial_refinement: bool,
    pub explain_with_counterexample: bool,
}

pub fn trace_refines<Spec, R>(
    spec: &Spec,
    map: &R,
    trace: &[ObservedEvent<R::ImplInput, R::ImplOutput, R::ImplState>],
    cfg: &TraceRefinementConfig,
) -> Result<TraceRefinementWitness<Spec>, TraceRefinementError>
where
    Spec: FrontendSpec + TemporalSpec,
    R: RefinementMap<Spec>;
```

### 5.3.1 実装方針

これは TLC ベースの trace validation の発想と同様に、**observed trace を制約として与えた constrained model checking** として実装する。<sup>[8]</sup>

高レベルには次を解く。

1. 初期抽象状態 `s0` が spec の初期状態集合に入る。
2. 各観測イベント `ei` について、candidate action のいずれかと hidden step の有限列が存在し、観測された before/after の抽象像と整合する。
3. 必要なら出力制約も満たす。
4. 必要なら fairness/liveness に関する制約も付加する。

初期実装では explicit backend を使った constrained search で十分である。symbolic backend はその後 `SpecCore` を直接使う BMC path と接続する。

### 5.3.2 不完全トレース

trace validation 論文では、完全状態ではなく spec 変数の更新のみを記録した不完全トレースでも有効に検証できることが示されている。<sup>[8]</sup>

そのため `ObservedEvent` は将来的に

```rust
pub enum Observation<C> {
    Full(C),
    Partial(Vec<FieldConstraint>),
}
```

へ拡張可能な形にしておく。

## 5.4 checker front door を explicit / symbolic で分離する

checker front door の分離は Phase 3 で実装済みであり、explicit と symbolic は型境界ごとに分離されている。<sup>[3]</sup>

これを次のように分ける。

```rust
pub struct ExplicitModelChecker<'a, T: CheckerSpec>(...);
pub struct SymbolicModelChecker<'a, T: CheckerSpec>(...);

impl<'a, T> ExplicitModelChecker<'a, T>
where
    T: CheckerSpec,
    T::State: PartialEq + FiniteModelDomain + Send + Sync,
    T::Action: PartialEq + Send + Sync,
{ ... }

impl<'a, T> SymbolicModelChecker<'a, T>
where
    T: CheckerSpec,
    T::Action: PartialEq + Send + Sync,
{ ... }
```

cleanup では public `ModelChecker` façade も削除し、workspace 全体を `ExplicitModelChecker` / `SymbolicModelChecker` へ移行した。

## 5.5 sound reduction と heuristic reduction を分離する

旧 API では reachable-graph dedup / branch pruning を `with_state_abstraction` と `with_por` で公開していたが、Phase 4 でそれらは `HeuristicReduction` へ統合された。<sup>[1][11]</sup>

このままでは、意味論保存な reduction と単なる探索ヒューリスティックが区別されない。

そのため API を次の二層に分ける。

```rust
pub enum ReductionMode<S, A> {
    Sound(SoundReduction<S, A>),
    Heuristic(HeuristicReduction<S, A>),
}

pub struct SoundReduction<S, A> {
    pub symmetry: Option<VerifiedSymmetry<S>>,
    pub quotient: Option<VerifiedStateQuotient<S>>,
    pub por: Option<VerifiedPor<S, A>>,
}

pub struct HeuristicReduction<S, A> {
    pub state_projection: Option<HeuristicStateProjection<S>>,
    pub action_pruning: Option<HeuristicActionPruning<S, A>>,
}
```

そして `ModelCheckResult` / `Counterexample` / docs 出力に **soundness tier** を載せる。

- `Exact`
- `SoundReduced`
- `Heuristic`

これにより、ユーザは「これは exact result か」「探索補助であり soundness を弱めるか」を見分けられる。

## 5.6 `nirvash-proof` を pretty export と sound export に分離する

`nirvash-proof` は現在 `PrettySpecExporter` と `SoundProofExporter` を同一 crate に持ち、前者が可読 dump、後者が sound obligation export を担う。<sup>[5]</sup>

本 RFC では次を提案する。

### 5.6.1 `PrettySpecExporter`

目的は可読な dump、デバッグ、レビューである。旧 `TlaModuleExporter` の責務はここへ移した。

### 5.6.2 `SoundProofExporter`

目的は proof obligation export である。対象 fragment を明示し、未対応 fragment は fail-closed とする。初期対象は

- invariants
- step simulation
- non-temporal state/action fragment

に限定する。

この方針は、TLAPS が proof manager により proof obligations を backend provers に分配し、標準 safety proof と step simulation を扱うという構造とも整合する。<sup>[10]</sup>

## 5.7 Rust 検証アダプタを追加する

`nirvash-conformance` の refinement API を、Rust の既存検証ツールへ接続する。

### 5.7.1 `proptest-state-machine`

逐次 API の SUT と `nirvash` spec を並走させ、最小 failing trace を得るアダプタを提供する。

```rust
pub fn run_proptest_state_machine<Spec, R, Sut>(...) -> TestCaseResult;
```

`proptest` の state machine testing は abstract reference state machine と SUT を比較し、壊れる transition sequence を縮約する用途に適している。<sup>[12]</sup>

### 5.7.2 `Kani`

小さい有限領域の pure / semi-pure コアには `#[kani::proof]` harness を自動生成または補助生成する。

```rust
#[kani::proof]
fn step_refines_insert() {
    let before = kani::any::<ImplState>();
    let input = kani::any::<Input>();
    kani::assume(concrete_inv(&before));
    let after = sut_step(before.clone(), input.clone());
    assert!(step_refines_relation(&SPEC, &MAP, &before, &input, &(), &after, &AUX).is_ok());
}
```

Kani は proof harness を最小検証単位として扱う bit-precise model checker である。<sup>[13]</sup>

### 5.7.3 `loom`

並行実装では実装イベント列を `ObservedEvent` へ変換し、各 schedule について `trace_refines` を走らせる。

```rust
loom::model(|| {
    let trace = run_concurrent_scenario_and_collect_trace();
    assert!(trace_refines(&SPEC, &MAP, &trace, &CFG).is_ok());
});
```

`loom` は C11 memory model 下で concurrent execution の順序を繰り返し入れ替えて探索する。<sup>[14]</sup>

## 5.8 `SpecCore` を checker の正本へさらに寄せる

本 RFC の主眼は code verification だが、その前提として `SpecCore` の source-of-truth 化を一段進める。

具体的には、

- `CheckerSpec::core() -> &SpecCore` を追加する
- direct SMT path と proof export は `SpecCore` を第一入力にする
- `FrontendSpec` / `TransitionProgram` は lowering source として残す
- trace refinement backend も可能な限り `SpecCore` から制約を組む

という方針をとる。

これは `nirvash-ir` がすでに `WF/SF`、`[A]_v` 相当の `BoxAction` / `AngleAction` を持っていることとも整合する。<sup>[6]</sup>

## 6. 詳細設計

## 6.1 検証モード

`nirvash-conformance` は次の 4 モードを持つ。

1. `InitRefinement`
2. `StepRefinement`
3. `TraceRefinement`
4. `TraceRefinementWithTemporalChecks`

`TraceRefinementWithTemporalChecks` は trace が safety だけでなく fairness/liveness assumption と両立するかを確認するモードである。ただし初期実装では optional にする。

## 6.2 返却値

単なる `bool` や panic ではなく、diagnostic を持つ result を返す。

```rust
pub enum RefinementError<S, A> {
    InitialStateMismatch,
    NoMatchingAbstractAction,
    NoMatchingAbstractSuccessor,
    OutputMismatch,
    HiddenStepBudgetExceeded,
    TemporalAssumptionViolated,
    HeuristicReductionActive,
    Backend(ModelCheckError),
}

pub struct StepRefinementWitness<S, A> {
    pub abstract_before: S,
    pub chosen_action: A,
    pub abstract_after: S,
    pub hidden_steps: usize,
}

pub struct TraceRefinementWitness<S, A> {
    pub abstract_states: Vec<S>,
    pub abstract_actions: Vec<Option<A>>,
    pub hidden_step_counts: Vec<usize>,
}
```

これにより、失敗時の counterexample が「なぜ失敗したか」を説明できる。

## 6.3 hidden/stuttering step の扱い

初期版では、各 observed event の前後に有限個の hidden/stuttering step を挿入できるモデルに限定する。

```text
r(c_i) --(hidden/stutter)*--> s_i --a_i--> s'_i --(hidden/stutter)*--> r(c_{i+1})
```

`hidden_step` 判定は refinement map が持つ。`allow_stuttering` と `max_hidden_steps_per_event` は config で制御する。

## 6.4 nondeterministic output の扱い

`ProtocolConformanceSpec` は現状 `ExpectedOutput` を持つが、将来的には出力 refinement も relation にする。

```rust
fn output_refines(
    &self,
    action: &Spec::Action,
    concrete_output: &ImplOutput,
    aux: &AuxState,
) -> bool;
```

これにより、レスポンス本文の一部だけが spec に現れるケース、乱数や時間の影響を auxiliary state へ退避するケースを扱える。

## 7. 互換性

- `assert_step_refinement` は削除し、`step_refines_summary` / `step_refines_relation` を正本にした。
- `ModelChecker` façade は削除し、`ExplicitModelChecker` / `SymbolicModelChecker` のみを public checker front door にした。
- `with_state_abstraction` / `with_por` は削除し、`HeuristicReduction` / `SoundReduction` 側へ統合した。
- `nirvash-proof` は crate 名を維持してよいが、module を `pretty` と `sound` に分ける。

## 8. 実装計画

### Phase 1

#### Phase 1A

- `nirvash-conformance` に `RefinementMap` trait を追加
- `step_refines_relation` を実装し、nondeterministic successor を relation として受理する
- `StepRefinementWitness` / `StepRefinementError` を導入し、result-returning API を正本にする

#### Phase 1B

- [x] `enabled_from_summary` を relation-aware に整理する
- [x] `step_refines_summary` を追加し、`StepRefinementWitness` / `StepRefinementError` を既存 conformance helper と整合させる

#### Phase 1C

- [x] code witness / generated macro harness の deterministic 仮定を relation API へ追従させる
- [x] `code_tests` / `code_witness_tests` の integration test と trybuild fixture を追加し、nondeterministic spec を誤って reject しない境界へ移行する

### Phase 2

- [x] `trace_refines` を explicit backend により実装
- [x] `ObservedEvent` と witness/result 型を導入
- [x] 反例整形を追加

### Phase 3

- [x] `ExplicitModelChecker` / `SymbolicModelChecker` を導入
- [x] public `ModelChecker` façade を削除し、workspace を typed checker へ移行
- [x] symbolic path から `FiniteModelDomain` 要件を外す

### Phase 4

- [x] sound / heuristic reduction の API 分離
- [x] `ModelCheckResult` に soundness tier を追加

### Phase 5

- [x] `proptest-state-machine` adapter を追加
- [x] `Kani` harness helper を追加
- [x] `loom` trace adapter を追加

### Phase 6

- [x] `PrettySpecExporter` と `SoundProofExporter` を分離
- [x] obligation fragment を明文化
- [x] unsupported fragment を fail-closed

## 9. 想定ワークフロー

### 9.1 逐次 API

1. `nirvash` で abstract spec を書く
2. `RefinementMap` を実装する
3. `proptest-state-machine` adapter で public API 列を生成する
4. 失敗時は `TraceRefinementWitness` と最小 failing sequence を比較する

### 9.2 有限領域コア

1. `Kani` harness を書く
2. input/state を `kani::any()` で生成する
3. `step_refines_relation` または短い `trace_refines` を assert する

### 9.3 並行データ構造 / actor runtime

1. `loom` で実行順序を列挙する
2. schedule ごとに実装イベントトレースを収集する
3. `trace_refines` を実行する
4. 必要なら `AuxState` に linearization bookkeeping を持たせる

## 10. 代替案

### 10.1 旧 `assert_step_refinement` を拡張し続ける

一見簡単だが、deterministic equality 前提が深く残るため、nondeterminism や hidden step を自然に扱えない。

### 10.2 すべてを `Kani` で検証する

bounded bug finding には強いが、分散・並行・長期トレース・unsafe proof まで一つで担うのは不自然である。`nirvash` 側の refinement API があれば、Kani はその一つの backend で済む。

### 10.3 TLAPS 相当をすぐ再実装する

proof management の実装コストが高く、本 RFC の主眼である spec-to-code verification より優先度が低い。

## 11. 欠点とリスク

1. `RefinementMap` 設計が抽象的すぎるとユーザ負担が大きい。
2. trace validation は hidden step 探索が爆発しやすい。
3. `loom` と `trace_refines` の組み合わせはテスト時間が長くなりうる。
4. `loom` と reduction/export の組み合わせは導入時の契約整理コストが高い。

これらに対しては、

- good default config
- budget と timeout
- minimal adapter template
- representative examples

で対処する。

## 12. 結論

再編後の `nirvash` は、formal frontend / core IR / lowering / checker / backend / conformance / proof export の分離という点で、すでに良い基盤になっている。<sup>[1]</sup>

次に必要なのは、モデル検査機能を増やすことそのものではなく、**spec と実 Rust 実装をどう結びつけるかを、refinement mapping と trace validation の観点から明文化すること**である。Abadi/Lamport の refinement mapping、TLA+ の trace validation、Kani / proptest / loom の各ツールは、そのための十分に強い理論的・実務的基盤を与えている。<sup>[7][8][9]</sup>

本 RFC を実装すれば、`nirvash` は「TLA+ 風の Rust-native spec/checker stack」から一段進み、**spec-to-code verification を中核に持つ Rust-native formal platform** になる。

## 参考文献

[1] `nirvash` README（crate split と backend semantics）  
https://github.com/yieldspace/imago/tree/codex/tla-formal-controls/crates/nirvash

[2] `nirvash-lower`（`LoweredSpec`, `TemporalSpec`, `CheckerSpec`）  
https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-lower/src/lib.rs

[3] `nirvash-check`（typed checker front door と backend 分離）  
https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-check/src/lib.rs

[4] `nirvash-conformance`（`step_refines_summary`, `step_refines_relation`, `trace_refines`）  
https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-conformance/src/lib.rs

[5] `nirvash-proof`（TLA exporter / SMT-LIB exporter）  
https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-proof/src/lib.rs

[6] `nirvash-ir`（`SpecCore`, `FairnessDecl`, stringly node 群）  
https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-ir/src/lib.rs

[7] Martín Abadi, Leslie Lamport, *The Existence of Refinement Mappings*  
https://www.microsoft.com/en-us/research/publication/the-existence-of-refinement-mappings/

[8] Horatiu Cirstea, Markus A. Kuppe, Benjamin Loillier, Stephan Merz, *Validating Traces of Distributed Programs Against TLA+ Specifications*  
https://arxiv.org/abs/2404.16075

[9] Tool references
- Proptest state machine testing: https://proptest-rs.github.io/proptest/proptest/state-machine.html
- Kani: https://model-checking.github.io/kani/
- Loom: https://docs.rs/loom/latest/loom/

[10] Kaustuv Chaudhuri, Damien Doligez, Leslie Lamport, Stephan Merz, *Verifying Safety Properties With the TLA+ Proof System*  
https://arxiv.org/abs/1011.2560
