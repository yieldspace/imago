# RFC 0003: `nirvash` における IR-Driven Verification Kernel、Constrained Trace Refinement、Certificate-Based Trust Boundary

- Status: Draft
- Created: 2026-03-14
- Target: `nirvash` の次段階アーキテクチャ
- Author: OpenAI

## 0. 要約

本 RFC は、現在の `nirvash` を「frontend/core/lower/check/backends/conformance/proof」に分離したうえで、次に残っている 3 つの seam を埋める提案である。

1. `SpecCore` を checker / proof / symbolic backend の**意味論の正本**にする。
2. `trace_refines` を explicit candidate trace の一致判定から、**constrained model checking ベースの一般化された trace refinement**へ引き上げる。
3. `Verified*` や `SoundProofExporter` が暗黙に背負っている trust boundary を、**証明書 (`ProofCertificate`) と trust tier**として型に出す。

本 RFC では、Abadi/Lamport の refinement mapping、TLA+ trace validation を constrained model checking に落とす近年の手法、および Rust 向けの `proptest-state-machine`、Kani、loom、Verus、RefinedRust を、同一の refinement モデルの上に接続する。これにより `nirvash` は「仕様を書く DSL」から、「仕様を正本として Rust 実装を検証する基盤」へ進む。

## 1. 背景

現在の `nirvash` は、README にあるとおり `nirvash` / `nirvash-ir` / `nirvash-lower` / `nirvash-check` / `nirvash-backends` / `nirvash-conformance` / `nirvash-proof` へ分割され、明確な crate split を持つ。README では `nirvash-ir` を backend 非依存の `SpecCore` と位置づけ、`nirvash-check` を explicit / symbolic の typed checker front door、`nirvash-conformance` を witness / runtime binding / refinement assert、`nirvash-proof` を `PrettySpecExporter` と `SoundProofExporter` として公開している。  
参照: [R1], [R2]

この再編により、少なくとも次の改善はすでに入っている。

- `LoweredSpec` が `SpecCore` を保持する。
- `ExplicitModelChecker` と `SymbolicModelChecker` が front door で分離され、explicit 側だけが `FiniteModelDomain` を要求する。
- `RefinementMap` と `step_refines_relation` が入り、summary equality 依存だった step-level conformance が relation-based に拡張された。
- `TrustTier = Exact | ClaimedReduction | Heuristic` が API surface に現れ、exact 検査と reduction/heuristic が区別された。  
  参照: [R3], [R4], [R5]

一方で、レビューの観点からは、次の 4 点がまだ architecture 上の未解決課題として残っている。

### 1.1 `SpecCore` がまだ checker の正本ではない

`LoweredSpec` は `pub core: SpecCore` を持つ一方で、`symbolic_artifacts()` と `executable()` を別に公開している。現状の public boundary では、`SpecCore` は存在していても、checker や backend が常に `core` を正本として読むとは限らない。  
参照: [R3]

### 1.2 `nirvash-ir` はまだ checker kernel としては浅い

`FairnessDecl::WF { view, action } / SF { view, action }` は良いが、`Quantified { domain: String, body: String }`、`Match { value: String, pattern: String }`、`Opaque(String)` など stringly な節点が残っている。これは interchange/export IR としては許容できるが、checker / proof の正本としては弱い。  
参照: [R6]

### 1.3 trace refinement が「候補 trace との完全一致」に寄っている

`RefinementMap` 自体は step-level で relation-based だが、`ObservedEvent` は `Action { action, output } | Stutter` に限定されており、trace matching は candidate trace の state length、step length、`loop_start` 一致を要求する。これは sequential deterministic API の validation には有効だが、hidden step、linearization point、部分観測、部分状態観測、非同期 invoke/return を持つ実装には狭い。  
参照: [R7], [R8], [R9]

### 1.4 trust boundary が still implicit である

`SymmetryReduction` / `StateQuotientReduction` / `PorReduction` は実装上「関数ポインタ + obligation の束」であり、obligation の自動 discharge は存在しない。また `SoundProofExporter` は supported fragment の obligation を export するが、temporal property と fairness を reject する。したがって「Verified」「Sound」という名前は現時点ではやや強い。  
参照: [R10], [R11]

## 2. 問題設定

本 RFC が扱う問題は、次の一文で要約できる。

> `nirvash` はすでに優れた formal frontend と checker stack だが、**IR が verification kernel になっていないこと、trace refinement が constrained checking に上がっていないこと、trust boundary が型に出ていないこと**により、spec-to-code verification の核としてはまだ途中段階にある。

この問題は、文献側から見ると自然である。Abadi/Lamport の refinement mapping は、下位実装状態から上位仕様状態への写像を中心に、step と trace の対応付けを行う。必要なら history / prophecy などの補助変数を加えて refinement mapping を成立させる。  
参照: [P1]

また、TLA+ trace validation の近年の仕事では、実装トレースと高水準 TLA+ 仕様の照合を、**constrained model checking** として扱う。そこでは、完全な abstract state を全て記録するのではなく、仕様変数の一部更新や不完全な観測だけを trace に含め、checker が欠落情報を再構成する。  
参照: [P2]

したがって `nirvash` でも、step-level の refinement relation に加えて、partial/incomplete trace を扱う constrained checking と、補助状態を持つ refinement map が必要である。

## 3. 目標

本 RFC の目標は次の 5 つである。

1. `SpecCore` を checker / proof / symbolic backend の**正本**にする。
2. stringly な IR を直ちに全廃せずとも、checker/proof が依存できる**正規化済み core**を導入する。
3. `trace_refines` を generic refinement map と partial observation を扱える trace-level API に拡張する。
4. reduction / proof export / external verifier の trust basis を `ProofCertificate` と tier で明示する。
5. `proptest-state-machine`、Kani、loom、Verus、RefinedRust を、共通の refinement モデルの上に接続する。

## 4. 非目標

本 RFC の非目標は次のとおりである。

- TLA+ 全構文の完全な re-parser / re-checker をこの RFC だけで完成させること。
- TLAPS 相当の proof assistant を Rust で全面再実装すること。
- すべての Rust 実装を単一ツールだけで証明すること。
- current explicit candidate matching を即時撤廃すること。

本 RFC は、**現行実装を壊さずに verification kernel を強くする**ことを目的とする。

## 5. 提案

### 5.1 `SpecCore` の正本化と `NormalizedSpecCore`

#### 5.1.1 提案

`LoweredSpec` を current shape のまま残しつつ、`SpecCore` と `NormalizedSpecCore` を意味論の中心に据える。

```rust
pub struct LoweredSpec<'a, S, A> {
    pub core: SpecCore,
    normalized_core: OnceCell<Result<Arc<NormalizedSpecCore>, CoreNormalizationError>>,
    symbolic_artifacts: SymbolicArtifacts<S, A>,
    executable: ExecutableSemantics<'a, S, A>,
    // ...
}

impl<'a, S, A> LoweredSpec<'a, S, A> {
    pub const fn core(&self) -> &SpecCore { &self.core }
    pub fn normalized_core(&self) -> Result<&NormalizedSpecCore, CoreNormalizationError> { /* ... */ }
}
```

`NormalizedSpecCore` は checker/proof が依存する正規化済み AST であり、次を満たす。

- binders / domains / bodies が構造化されている
- `Opaque(String)` や stringly `Quantified` を残さない
- backend が fail-closed できる fragment metadata を持つ

```rust
pub struct FragmentProfile {
    pub has_opaque_nodes: bool,
    pub has_stringly_quantifiers: bool,
    pub has_temporal_props: bool,
    pub has_fairness: bool,
    pub symbolic_supported: bool,
    pub proof_supported: bool,
}
```

#### 5.1.2 根拠

現状、`LoweredSpec` は `core` を持ちながら、`symbolic_artifacts()` と `executable()` を parallel に保持している。これ自体は pragmatic だが、verification kernel を `SpecCore` に置くなら、direct SMT path と proof export path は **必ず core から出発**すべきである。  
参照: [R3]

また、`SpecCore` には `WF/SF(view, action)` がすでに存在するが、他の節点には stringly なノードが残る。したがって、既存 `SpecCore` を捨てるのではなく、**checker/proof 専用の正規化結果**を重ねるのが現実的である。  
参照: [R6]

#### 5.1.3 期待効果

- symbolic backend / proof exporter / doc graph が同一の core を見る
- unsupported fragment を `FragmentProfile` で早期に fail-closed できる
- operational DSL 由来の `executable` と意味論の正本が分離される

### 5.2 fairness authoring surface の追加

IR 側には `FairnessDecl::WF { view, action } / SF { view, action }` があるが、authoring surface では `view` を明示する道が薄い。したがって、legacy `TemporalSpec::fairness()` を残しつつ、core fairness を直接返す surface を追加する。

```rust
pub trait CoreTemporalSpec: FrontendSpec {
    fn core_fairness(&self) -> Vec<FairnessDecl> {
        Vec::new()
    }
}
```

legacy fairness は lowering で `ViewExpr::Vars` に落とす default とし、core fairness を与えた spec ではそれを優先する。

#### 根拠

IR では `view` が first-class なのに、authoring surface で書けないのは中途半端である。現在の実装は view を IR に導入する段階までは進んでいるため、次はそれを authoring に引き上げるべきである。  
参照: [R6]

### 5.3 generic trace refinement の導入

#### 5.3.1 新しい trait

現行の `RefinementMap` は step-level では十分有用だが、trace-level では `AuxState` の逐次更新、partial observation、hidden step の扱いが不足している。そこで新しく `TraceRefinementMap` を導入する。

```rust
pub trait TraceRefinementMap<Spec: FrontendSpec> {
    type ImplState;
    type ImplEvent;
    type AuxState: Clone;

    fn init_aux(&self, initial: &Self::ImplState) -> Self::AuxState;

    fn next_aux(
        &self,
        before: &Self::ImplState,
        event: &Self::ImplEvent,
        after: &Self::ImplState,
        aux: &Self::AuxState,
    ) -> Self::AuxState;

    fn abstract_state(&self, state: &Self::ImplState, aux: &Self::AuxState) -> Spec::State;

    fn candidate_actions(
        &self,
        before: &Self::ImplState,
        event: &Self::ImplEvent,
        after: &Self::ImplState,
        aux: &Self::AuxState,
    ) -> Vec<Spec::Action>;

    fn output_matches(
        &self,
        spec: &Spec,
        action: &Spec::Action,
        abstract_before: &Spec::State,
        abstract_after: &Spec::State,
        event: &Self::ImplEvent,
        aux: &Self::AuxState,
    ) -> bool;

    fn hidden_step(
        &self,
        _before: &Self::ImplState,
        _event: &Self::ImplEvent,
        _after: &Self::ImplState,
        _aux: &Self::AuxState,
    ) -> bool {
        false
    }
}
```

現行 `RefinementMap` は `TraceRefinementMap` の restricted form として残し、後方互換ラッパで持ち上げる。

#### 5.3.2 event model の拡張

現行の `ObservedEvent` は `Action | Stutter` に限られる。これを次へ広げる。

```rust
pub enum ObservedEvent<A, O, I = ()> {
    Invoke { input: I },
    Return { output: O },
    Action { action: A, output: O },
    Internal,
    Stutter,
}
```

Sequential deterministic API では `Action` だけで十分だが、concurrent / async 実装では `Invoke` / `Return` / `Internal` が必要になる。

#### 5.3.3 observation model の拡張

現在の `ObservedTrace` は「state 列 + event 列 + loop_start」という完全観測前提に近い。これを partial/incomplete 観測へ広げる。

```rust
pub enum StateObservation<S> {
    Full(S),
    Partial(S),
    Unknown,
}

pub struct ObservedTrace<S, E> {
    pub states: Vec<StateObservation<S>>,
    pub events: Vec<E>,
    pub loop_start: Option<usize>,
}
```

`Partial(S)` は summary/projection を意味し、checker 側が欠落情報を補完する。

#### 根拠

Abadi/Lamport の refinement mapping は、必要に応じて auxiliary variable を導入して抽象仕様と具体実装を結びつける枠組みを与える。`AuxState` を trace-level で進めるのは、この系譜に一致する。  
参照: [P1]

また、TLA+ trace validation は、完全 state ではなく「仕様変数の一部更新」だけを観測し、それを constrained model checking で補完する。`nirvash` の summary-based runtime conformance を一般化する際、この設計は直接の参照先になる。  
参照: [P2]

### 5.4 `TraceRefinementEngine` と `constrained_trace_refines`

現行の trace refinement は explicit candidate trace との一致判定が本体である。これを次の 3 エンジンへ分割する。

```rust
pub enum TraceRefinementEngine {
    ExplicitCandidate,
    ExplicitConstrained,
    SymbolicConstrained,
}

pub struct TraceRefinementConfig {
    pub engine: TraceRefinementEngine,
    pub max_hidden_steps_between_observations: usize,
    pub require_total_observation: bool,
    pub allow_lasso: bool,
}
```

新しい front door は次の形とする。

```rust
pub fn constrained_trace_refines<Spec, R>(
    spec: &Spec,
    map: &R,
    observed: &ObservedTrace<R::ImplState, R::ImplEvent>,
    config: TraceRefinementConfig,
) -> Result<TraceRefinementWitness<Spec::State, Spec::Action>, TraceRefinementError<Spec::State, Spec::Action>>
where
    Spec: FrontendSpec,
    R: TraceRefinementMap<Spec>;
```

#### 5.4.1 engine semantics

- `ExplicitCandidate`  
  現在の実装を fast path として残す。完全観測トレースに対して最速。

- `ExplicitConstrained`  
  観測トレースを constraint として、explicit product search を行う。hidden step を bounded に挿入できる。

- `SymbolicConstrained`  
  `SpecCore` / `NormalizedSpecCore` から、`s0, s1, ...` と observation constraint を SMT に落とす。partial trace、部分観測、hidden step、bounded lasso を扱う。

#### 5.4.2 SMT 側の基本形

観測点 `obs_i` の間に最大 `k` 個の hidden step を許すなら、典型的には次の形を使う。

```text
Init(s_0)
ObsState(0, s_0)
StepOrHidden(s_0, s_1)
...
ObsEvent(0, s_j, a_j, s_{j+1})
ObsState(1, s_{j+1})
...
```

このとき `ObsState` は partial summary constraint、`ObsEvent` は observed action/output/invoke-return との整合制約である。

#### 根拠

TLA+ trace validation の文献は、まさにこの問題を constrained model checking として扱っている。現在の `trace_refines_summary_with_label` が要求する「長さ一致」「loop_start 一致」「candidate trace と prefix state 一致」は、完全観測・完全同期の特殊ケースにすぎない。  
参照: [R9], [P2]

### 5.5 trust boundary の型化

#### 5.5.1 `ProofCertificate`

現在の `SymmetryReduction` / `StateQuotientReduction` / `PorReduction` は、proof obligation を添付できるが、その obligation が discharge 済みかどうかは型に出ていない。これを次で明示する。

```rust
pub enum ProofBackendId {
    Tlaps,
    Smt,
    Kani,
    Verus,
    RefinedRust,
    External(String),
}

pub struct ProofCertificate {
    pub backend: ProofBackendId,
    pub obligation_hash: String,
    pub artifact_hash: String,
    pub artifact_path: Option<PathBuf>,
}
```

#### 5.5.2 claim と certificate の分離

`Verified*` を直ちに消すのではなく、semantic には次の 2 層へ分ける。

```rust
pub struct ReductionClaim<T> {
    pub value: T,
    pub obligations: Vec<ProofObligation>,
}

pub struct Certified<T> {
    pub value: T,
    pub certificate: ProofCertificate,
}
```

これにより `SymmetryReduction` は deprecated alias とし、将来的に

- `ClaimedSymmetry`
- `CertifiedSymmetry`

へ移行できる。

#### 5.5.3 tier の再定義

`TrustTier` は current implementation では useful だが、「claimed reduction」と「certificate 付き reduction」を区別しない。したがって次へ変更する。

```rust
pub enum TrustTier {
    Exact,
    CertifiedReduction,
    ClaimedReduction,
    Heuristic,
}
```

既存 `TrustTier` は deprecated alias とする。

#### 根拠

現在の reduction types は obligation を保持するが、その discharge までは表現しない。したがって「Verified」という命名は強い。proof/export 側も `SoundProofExporter` が temporal/fairness を reject する supported fragment exporter として振る舞っているため、信頼境界は still implicit である。  
参照: [R10], [R11]

### 5.6 `nirvash-proof` の二段化

`nirvash-proof` は次の 2 段に整理する。

1. `ProofBundleExporter`  
   `SpecCore` / `NormalizedSpecCore` と reduction claim から obligations と exported artifacts を生成する。

2. `ProofDischarger`  
   TLAPS / SMT / Kani / Verus / RefinedRust / external script 等を通じて obligation を discharge し、`ProofCertificate` を返す。

```rust
pub struct ProofBundle {
    pub obligations: Vec<ProofObligation>,
    pub exported_artifacts: Vec<ExportedArtifact>,
}

pub trait ProofDischarger {
    fn discharge(&self, bundle: &ProofBundle) -> Result<ProofCertificate, ProofDischargeError>;
}
```

現行 `PrettySpecExporter` はこの構造の pretty/export 部分に自然に収まり、現行 `SoundProofExporter` は supported fragment checked exporter へ再定義される。

## 6. Rust 向け検証スタックとの接続

本 RFC では、Rust 向けの複数の verification/testing tool を、**同一の refinement interface** に接続する。

### 6.1 `proptest-state-machine`

`proptest-state-machine` は、抽象 reference state machine と SUT を並走させ、反例シーケンスを shrink できる。これは sequential API の spec-to-code divergence を見つける入口として最も費用対効果が高い。  
参照: [P3]

提案:

- `nirvash-conformance-proptest` feature を追加する
- `TraceRefinementMap` または restricted `RefinementMap` から `ReferenceStateMachine` / `StateMachineTest` を自動生成する
- 失敗時は `TraceRefinementWitness` と shrink 済み command sequence を返す

### 6.2 Kani

Kani は proof harness を用いる Rust 向けモデルチェッカであり、モデル検査により safety/correctness を自動検査し、counterexample を返せる。bounded な pure / semi-pure core に対して、step refinement を全件探索するのに適している。  
参照: [P4]

提案:

- `nirvash-conformance-kani` feature を追加する
- `step_refines_relation` から Kani proof harness を生成する
- harness を discharge した結果を `ProofCertificate { backend: Kani, ... }` として取り込めるようにする

### 6.3 loom

loom は C11 memory model の下で concurrent execution の順序を網羅的に近く探索する test tool である。Kani 自身は concurrency が未充足であるため、並行実装の schedule exploration には loom を用い、その観測 event trace を `constrained_trace_refines` に流すのが自然である。  
参照: [P4], [P5]

提案:

- `nirvash-conformance-loom` feature を追加する
- 各 loom schedule から `ObservedTrace` を組み立てる helper を提供する
- `Invoke` / `Return` / `Internal` event を trace engine に流す

### 6.4 Verus / RefinedRust

Verus は Rust 言語内で仕様と proof を記述し、線形 ghost type を用いて Rust プログラムの正しさを検証する。RefinedRust は Coq 上で sound な refinement type system を持ち、安全/unsafe Rust の機能的正しさを対象とする。これらは `nirvash` の checker を置き換えるものではなく、**危険な実装境界に対する stronger certificate source** として扱うべきである。  
参照: [P6], [P7]

提案:

- `ProofBackendId::Verus` / `ProofBackendId::RefinedRust` を追加する
- external proof artifact hash を `ProofCertificate` に取り込めるようにする
- reduction claim や code-level invariant の certificate source として利用する

## 7. 互換性

本 RFC は大きな再編ではあるが、互換性を次のように守る。

- `LoweredSpec { core, symbolic_artifacts, executable }` は維持する
- current `RefinementMap` / `step_refines_relation` は維持し、`TraceRefinementMap` への adapter を提供する
- current `ObservedEvent::Action | Stutter` は新 enum の subset として動かす
- current candidate-based `trace_refines` は `TraceRefinementEngine::ExplicitCandidate` に残す
- `Verified*` と `TrustTier` は deprecated alias として一定期間残す

## 8. 実装計画

### Phase 1: core の正本化

- `LoweredSpec::core()` / `normalized_core()` を追加
- symbolic backend を `normalized_core()` first へ寄せる
- `nirvash-proof` を `ProofBundleExporter` 化する

### Phase 2: trace refinement trait の一般化

- `TraceRefinementMap` を追加
- `ObservedEvent` を拡張
- `ObservedTrace` に partial observation を追加
- current summary API を compatibility wrapper 化する

### Phase 3: constrained engine 実装

- `ExplicitConstrained` を product search として実装
- `SymbolicConstrained` を direct SMT/BMC として実装
- partial trace / hidden step / bounded lasso を順次対応する

### Phase 4: trust boundary の明文化

- `ReductionClaim<T>` / `Certified<T>` / `ProofCertificate` を追加
- `TrustTier` を `TrustTier` へ置換
- reduction / proof export / external verifier を certificate で接続する

### Phase 5: tool adapter の統合

- `proptest` adapter
- Kani harness generator
- loom trace adapter
- Verus / RefinedRust certificate importer

## 9. 受け入れ基準

本 RFC は、次の条件を満たしたときに完了とみなす。

1. symbolic backend と proof exporter が `normalized_core()` から直接動作する。
2. partial observation を含む `ObservedTrace` に対し、`constrained_trace_refines` が witness または counterexample を返す。
3. `ObservedEvent` が `Invoke | Return | Internal | Action | Stutter` を扱える。
4. reduction claim と certificate の区別が API に現れる。
5. `proptest` / Kani / loom adapter が同じ refinement model に乗る。
6. current exact candidate engine は regress せず fast path として残る。

## 10. 欠点とトレードオフ

- `NormalizedSpecCore` を追加すると、IR の二重管理コストが増える。
- constrained trace checking は explicit candidate match より重い。
- certificate model を入れると API がやや複雑になる。
- Verus / RefinedRust integration は外部ツール依存を導入する。

ただし、これらはすべて「verification kernel を強くする」ためのコストであり、現在の曖昧さを減らす効果の方が大きい。

## 11. 代替案

### 11.1 現状の candidate trace matching を維持する

これは実装コストが最も低いが、partial trace、hidden step、invoke/return、linearization point を扱いにくい。TLA+ trace validation が constrained model checking を採用している理由と逆行する。  
参照: [P2]

### 11.2 `SpecCore` を export 専用 IR のままにする

これは operational DSL と backend の coupling を温存する。`nirvash` を spec-to-code verification kernel として説明しにくくなる。

### 11.3 `Verified*` の命名をそのまま維持する

利用者にとっては簡単だが、claim と certificate を区別できず、trust boundary が不透明なまま残る。

## 12. 未解決事項

- `NormalizedSpecCore` を `nirvash-ir` に置くか、別 crate に置くか。
- partial state observation を `StateObservation::Partial(Summary)` で十分とするか、predicate/constraint object にするか。
- `ProofCertificate` の hash 対象を obligation 単位にするか、bundle 単位にするか。
- `SoundProofExporter` の rename を直ちに行うか、deprecation を挟むか。
- fairness / temporal property を symbolic constrained trace engine の初期スコープへ入れるか、まず safety に限定するか。

## 13. 結論

現在の `nirvash` は、frontend / lower / check / backends / conformance / proof を分離した点で、すでにかなり良い位置にある。しかし、次の段階に進むには、**IR を verification kernel にし、trace refinement を constrained checking に上げ、trust boundary を証明書として型に出すこと**が必要である。

本 RFC は、そのための最小限かつ実装可能な道筋を与える。current implementation を壊さずに、`nirvash` を「Rust-native な TLA+/refinement ベースの spec-to-code verification 基盤」として一段引き上げることができる。

## 14. 参考文献

### Repository / current implementation

- [R1] `crates/nirvash/README.md` (crate split と backend contract)  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash/README.md>
- [R2] `crates/nirvash/README.md` の crate split と semantics セクション  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash/README.md#crate-split>
- [R3] `crates/nirvash-lower/src/lib.rs`: `LoweredSpec` が `core` と `symbolic_artifacts` / `executable` を併せ持つ  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-lower/src/lib.rs>
- [R4] `crates/nirvash-check/src/lib.rs`: `ExplicitModelChecker` と `SymbolicModelChecker` の分離  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-check/src/lib.rs>
- [R5] `crates/nirvash-conformance/src/lib.rs`: `RefinementMap` と `step_refines_relation`  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-conformance/src/lib.rs>
- [R6] `crates/nirvash-ir/src/lib.rs`: `SpecCore`, `FairnessDecl`, stringly quantifier / match / opaque nodes  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-ir/src/lib.rs>
- [R7] `crates/nirvash-conformance/src/lib.rs`: `ObservedEvent` が `Action | Stutter` に限定される  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-conformance/src/lib.rs>
- [R8] `crates/nirvash-conformance/src/lib.rs`: `trace_refines_summary_with_label` の shape / loop 一致前提  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-conformance/src/lib.rs>
- [R9] `crates/nirvash-conformance/src/lib.rs`: candidate trace ベースの error reporting  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-conformance/src/lib.rs>
- [R10] `crates/nirvash-lower/src/lib.rs`: `SymmetryReduction` / `StateQuotientReduction` / `PorReduction`  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-lower/src/lib.rs>
- [R11] `crates/nirvash-proof/src/lib.rs`: `SoundProofExporter` と supported fragment 制約  
  <https://github.com/yieldspace/imago/blob/codex/tla-formal-controls/crates/nirvash-proof/src/lib.rs>

### Papers / formal basis

- [P1] Martín Abadi, Leslie Lamport, *The Existence of Refinement Mappings*  
  <https://lamport.azurewebsites.net/pubs/abadi-existence.pdf>
- [P2] Horatiu Cirstea, Markus A. Kuppe, Benjamin Loillier, Stephan Merz, *Validating Traces of Distributed Programs Against TLA+ Specifications*  
  <https://arxiv.org/abs/2404.16075>
- [P3] Proptest documentation, *State Machine testing*  
  <https://proptest-rs.github.io/proptest/proptest/state-machine.html>
- [P4] Kani documentation, *Getting started*  
  <https://model-checking.github.io/kani/>
- [P5] loom documentation  
  <https://docs.rs/loom/latest/loom/>
- [P6] Andrea Lattuada et al., *Verus: Verifying Rust Programs using Linear Ghost Types*  
  <https://dl.acm.org/doi/10.1145/3586037>
- [P7] RefinedRust project page / paper  
  <https://plv.mpi-sws.org/refinedrust/>
