# RFC 0004: `code_tests` を import-first な Test-Synthesis Compiler に再定義する

- Status: Draft
- Author: OpenAI / ChatGPT
- Target: `nirvash`, `nirvash-lower`, `nirvash-check`, `nirvash-conformance`, `nirvash-proof`, `nirvash-macros`
- Type: Breaking change
- Created: 2026-03-14

## 0. 要約

本 RFC は、`nirvash` の `code_tests` / `code_witness_tests` 系を **単なるテスト補助マクロ** から、**spec から unit / concurrency / e2e の test harness・oracle・scheduler・replay artifact を合成する import-first な test-synthesis compiler** へ再定義する提案である。

設計の中心は次の 4 点である。

1. `code_tests` は個々の `#[test]` を大量展開するマクロではなく、**generated module** と **installer macro** と **runtime harness plan** を生成する。
2. ユーザーは基本的に **手書きの test case を書かない**。代わりに、spec・binding・必要最小限の seed/fixture override だけを書く。
3. 生成 engine は単一ではなく、**explicit suite generation / proptest online / Kani proof harness / trace validation / loom / shuttle** の multi-engine とする。
4. state の初期値や action 引数のベース入力は、`FiniteModelDomain`、`Default`、`Arbitrary`、spec guard からの boundary 抽出を優先し、足りないときだけ簡易 builder / attribute で補う。

この方向は、Spec Explorer における model からの test sequence と oracle の自動生成、on-the-fly testing、Modelator による TLA+ モデルからのテスト生成と Rust/Go 連携、TLA+ trace validation の constrained model checking 化、`proptest-state-machine` の sequential state-machine testing、`loom` / `shuttle` の並行テスト、Kani の concrete playback という既存の知見と整合する。Spec Explorer では model から test sequence と oracle が自動生成され、online testing では生成と実行が一体化される。Modelator は TLA+ を入力として自動列挙したテストを target language へ接続する。TLA+ trace validation は、実装トレースと spec の整合を constrained model checking として解き、完全状態ではなく spec 変数の更新だけの記録でもよい。`proptest-state-machine` は reference state machine に対する反例探索と縮約を提供するが、現時点では `sequential` のみを正式サポートする。`loom` は C11 memory model 下で schedule を系統探索し、`shuttle` は Loom より大きなケースに向く randomized / PCT scheduler を提供する。Kani は failing proof harness から concrete playback として Rust unit test を生成できる。[^specexplorer_intro][^specexplorer_otf][^modelator_docs][^trace_validation][^proptest_sm][^loom_docs][^shuttle_docs][^kani_playback]

本 RFC は **破壊的変更を許容** し、`code_tests` の public contract を次のように変える。

- 旧: 「proc macro で test を生やす DSL」
- 新: 「spec から generated artifacts と test harness plan を生成し、ユーザーはそれを import して local test を install する DSL」

## 1. 背景と問題設定

現行の `nirvash` は frontend / IR / lowering / checker / conformance / proof へよく分離されているが、実コード検証の最後の一歩、すなわち **spec から test を自動合成し、実装へつなげて回し切る** 部分はまだ薄い。

現在の延長線で `RefinementMap` に接続するだけでは、ユーザーは結局次のいずれかを書かされる。

- `proptest` 用の command generator / reference model / oracle
- `loom` / `shuttle` 用の schedule-aware harness
- e2e trace の収集と checker への接続コード
- failing trace の replay test

これは `nirvash` の狙い、すなわち **AI が生成する unit test や e2e test では取りこぼす挙動を、形式 spec から網羅的かつ自動的に埋める** という目的に対して不十分である。

モデルベーステストの文脈では、重要なのは「テストを書きやすくする」ことではなく、「**モデルからテストと oracle を自動生成する**」ことである。Spec Explorer はまさに test sequence と oracle をモデルから自動生成するツールとして設計されているし、Modelator も TLA+ モデルを入力にテストを自動列挙し、実装へ接続する。したがって、`nirvash` においても `code_tests` は helper macro ではなく **test compiler** でなければならない。[^specexplorer_intro][^modelator_docs]

同時に、テストの「自動性」は coverage criterion を明示しないと空虚になる。MBT の古典では state coverage、transition coverage、transition-pair / N-switch、loop / round-trip、boundary-oriented coverage などが区別され、coverage criterion ごとに見つかる欠陥が異なる。したがって `nirvash` は「全部自動生成する」と言うのではなく、「**選択した coverage goal と bound に対して test suite / online strategy / trace checker を自動合成する**」べきである。[^mbt_taxonomy][^coverage_survey]

## 2. 目標

### 2.1 主要目標

1. ユーザーが個々の unit test / e2e test / concurrency test を手書きしなくてもよいこと。
2. ユーザーが書くのは基本的に spec と binding と少量の override だけであること。
3. 生成されたものを **import して使う** フローを正本にすること。
4. state の初期値や action 引数のベース入力を、できるだけ自動推論し、必要なら簡単な builder で与えられること。
5. `cargo test` / `cargo kani` / `loom` / `shuttle` / e2e trace validation を同一 spec から派生させること。
6. 失敗時に replay artifact・縮約済み trace・Kani concrete playback test を回収できること。

### 2.2 副次目標

1. compile time を暴走させないこと。
2. generated test code の量を最小化し、heavy な探索や suite planning は runtime へ逃がすこと。
3. CI 上で deterministic replay ができること。
4. explicit / symbolic / runtime trace validation の結果が同じ artifact 形式へ収束すること。

## 3. 非目標

1. arbitrary な外部システムに対する完全ゼロ設定接続。
2. async / distributed / time / randomness / filesystem / network をすべて自動推論すること。
3. 全エンジンで同じ探索完全性を保証すること。
4. TLAPS 相当の完全証明を `code_tests` に持ち込むこと。
5. proc macro のみで巨大なテスト列挙をすべて source code 化すること。

## 4. 提案の要点

`code_tests` を次の意味に再定義する。

> `code_tests` は、spec と binding から generated module / harness plan / replay artifact schema を合成し、ユーザーは generated module を import して local test installer を 1 回呼ぶだけで unit / concurrency / e2e テスト群を有効化できる。

この提案では、旧来の `code_tests` / `code_witness_tests` は削除または compatibility layer に落とし、正本 API を以下へ置き換える。

- spec crate が生成する `generated` module
- binding derive / attribute
- seed / fixture profile builder
- local test installer macro
- optional `cargo nirvash materialize-tests` / `cargo nirvash replay` CLI

## 5. 破壊的変更

本 RFC は次の breaking changes を含む。

### 5.1 `code_tests` の意味変更

`#[code_tests(...)]` は「その場で test を展開する属性」ではなく、**generated companion module を作る属性**に変わる。既存利用はコンパイルエラーにしてよい。

### 5.2 `code_witness_tests` の削除

`code_witness_tests` は独立 macro としては廃止し、`generated::replay` / `generated::trace_validation` へ統合する。

### 5.3 runtime binding surface の統合

`ActionApplier` / `StateObserver` / witness 周辺 trait は compatibility shim に下げ、正本は `RuntimeBinding` / `TraceBinding` / `ProjectionBinding` の生成系へ移す。

### 5.4 test installation は local expansion に限定

Rust の `#[test]` は最終的に test crate 内へ存在する必要がある。よって、spec crate から「完成済み test 関数」を export するのではなく、**generated installer macro** を import して local test module へ展開するフローを正本とする。

## 6. 新しい全体像

## 6.1 二段生成モデル

本 RFC では test 生成を 2 段に分ける。

1. **compile-time metadata generation**  
   spec crate 側で generated metadata / typed plans / profile builders / installer macro を生成する。

2. **test-runtime synthesis**  
   `cargo test` / `cargo kani` / `loom` / `shuttle` 実行時に heavy な suite planning、trace enumeration、command generation、shrink/replay を行う。

この分離により、proc macro expansion の爆発を避ける。

## 6.2 生成される companion module

各 spec は `generated` module を持つ。

```rust
pub mod generated {
    pub mod prelude;
    pub mod bindings;
    pub mod seeds;
    pub mod profiles;
    pub mod plans;
    pub mod replay;
    pub mod install;
}
```

### `generated::prelude`

- spec に対応する型 alias
- default profiles
- default coverage presets
- installer macro 群

### `generated::bindings`

- derive / attribute から生成された `RuntimeBinding` adapter
- action name と impl method の対応表
- projection / observation helper

### `generated::seeds`

- `small()`, `boundary()`, `concurrent_small()`, `e2e_default()` などの builder
- type-based domain override API
- fixture override API

### `generated::profiles`

- engine + coverage + seed preset を束ねた高水準 profile

### `generated::plans`

- `GeneratedHarnessPlan`
- `CoverageGoal`
- `EnginePlan`

### `generated::replay`

- counterexample trace schema
- Kani playback / explicit replay / trace validation replay helper

### `generated::install`

- local test installer macro
- profile ごとの installer

## 7. ユーザーが書くもの

ユーザーが書くべきものは原則として次の 3 つだけにする。

1. **spec**  
   既存どおり `FrontendSpec` / `TemporalSpec` を書く。

2. **binding**  
   実装と spec action/state を結ぶ最小限の binding を書く。

3. **必要なら override**  
   seed / fixture / environment の不足分だけを 1 行から数行で補う。

### 7.1 最小形

```rust
use my_spec::generated::prelude::*;

#[derive(NirvashBinding)]
#[nirvash(spec = MySpec)]
pub struct MyRuntime {
    sut: MyService,
}

impl Default for MyRuntime {
    fn default() -> Self { Self { sut: MyService::new() } }
}

impl MyRuntime {
    fn put(&mut self, key: Key, value: Value) { self.sut.put(key, value) }
    fn get(&mut self, key: Key) -> Option<Value> { self.sut.get(key) }
}

nirvash::import_generated_tests! {
    spec = MySpec,
    binding = MyRuntime,
}
```

この形では、action variant 名と method 名が一致する限り、ユーザーは個別の test case を一切書かない。

### 7.2 少しだけ明示する形

```rust
use my_spec::generated::prelude::*;
use my_spec::generated::seeds::{small, boundary};

#[derive(NirvashBinding)]
#[nirvash(spec = MySpec)]
pub struct MyRuntime {
    sut: MyService,
}

impl MyRuntime {
    #[nirvash(action = Put)]
    fn exec_put(&mut self, key: Key, value: Value) -> Result<(), Error> {
        self.sut.exec_put(key, value)
    }

    #[nirvash(action = Get)]
    fn exec_get(&mut self, key: Key) -> Option<Value> {
        self.sut.exec_get(key)
    }
}

nirvash::import_generated_tests! {
    spec = MySpec,
    binding = MyRuntime,
    profiles = [
        small().with_seed::<Key>(["a", "b", "c"]),
        boundary().with_seed::<u64>([0, 1, 255, 256]),
    ],
}
```

### 7.3 何をユーザーに書かせないか

以下は原則 `nirvash` が自動生成する。

- proptest の command generator
- explicit suite の shortest trace / transition-tour planner
- Kani proof harness
- counterexample replay test
- trace validation driver
- loom / shuttle の schedule harness
- generated oracle

## 8. Binding surface の再設計

`RuntimeBinding` を正本にする。`derive` と attribute から生成し、旧 `ActionApplier` / `StateObserver` は compatibility layer に下げる。

```rust
pub trait RuntimeBinding<Spec: FrontendSpec> {
    type Sut;
    type Fixture;
    type Output;
    type Error;

    fn create(fixture: &Self::Fixture) -> Result<Self::Sut, Self::Error>;
    fn apply(sut: &mut Self::Sut, action: &Spec::Action) -> Result<Self::Output, Self::Error>;
    fn project(sut: &Self::Sut) -> ProjectedState<Spec::State>;
}
```

### 8.1 derive/infer 規則

`#[derive(NirvashBinding)]` は次の順で推論する。

1. method 名と action variant 名の一致
2. `#[nirvash(action = ...)]` attribute
3. signature の引数型と action payload 型の一致
4. `project()` は field 名と spec var 名の一致、または `#[nirvash(var = ...)]` で補う
5. fixture は `Default`、`new()`, `builder()`, `#[nirvash(init)]` を順に試す

この設計により、もっとも単純なケースではユーザーは `derive` と `Default` だけで済む。

## 9. Seed / Fixture / Environment の再設計

### 9.1 原則

ベース入力は **まず自動推論**し、足りないときだけ簡単に override できるようにする。

優先順位は次のとおり。

1. `FiniteModelDomain`
2. spec の guard / update / invariant から抽出した boundary 値
3. `Arbitrary` / `Strategy`
4. `Default` / singleton seed
5. 明示 `with_seed::<T>(...)` / attribute

### 9.2 自動 boundary 抽出

`nirvash-ir` / normalized spec から、以下を抽出して boundary seed 候補を作る。

- 比較式の定数 `x < N`, `x <= N`, `x == N`
- 長さ条件 `len(xs) == N`, `len(xs) < N`
- 集合 membership / cardinality の閾値
- enum variant の全値
- set / map / sequence 更新に現れる literal
- fairness / temporal guard に出る action subset

この pass を `BoundaryMining` と呼ぶ。`BoundaryMining` は deterministic であることを要求する。

### 9.3 生成される profile

すべての spec に対して、少なくとも次の profile builder を自動生成する。

- `small()`  
  小さな有限領域。unit / explicit / proptest の default。
- `boundary()`  
  guard 比較境界を優先した領域。
- `concurrent_small()`  
  2 thread / 短い history / 小さな domain。loom / shuttle の default。
- `e2e_default()`  
  trace validation 用。full state ではなく spec variable update の logging を前提。
- `soak()`  
  randomized / longer history。sound ではないがバグ発見向け。

### 9.4 ユーザー override API

override はできるだけ単純にする。

```rust
use my_spec::generated::seeds::small;

let p = small()
    .with_fixture(MyRuntime::default())
    .with_seed::<Key>(["a", "b", "c"])
    .with_seed::<u64>([0, 1, 1024]);
```

または attribute で書く。

```rust
#[derive(NirvashSeed)]
struct MySeedOverrides {
    #[nirvash(fixture)]
    fixture: MyRuntime,
    #[nirvash(values("a", "b", "c"))]
    key: Key,
    #[nirvash(boundary(0, 1, 1024))]
    value: u64,
}
```

### 9.5 environment seed

時間・乱数・ネットワーク等は `EnvironmentSeed` にまとめる。

```rust
pub struct EnvironmentSeed {
    pub clock_seed: u64,
    pub rng_seed: u64,
    pub io_mode: IoMode,
    pub network_mode: NetworkMode,
}
```

ユーザー override は profile builder に統合する。

```rust
let p = concurrent_small().with_rng_seed(7).with_clock_seed(42);
```

## 10. Coverage goal の正本化

`nirvash` は「全部テストする」という表現をやめ、`CoverageGoal` を first-class にする。

```rust
pub enum CoverageGoal {
    States,
    Transitions,
    TransitionPairs { k: usize },
    RoundTrips,
    GuardBoundaries,
    PropertyPrefixes,
    Goals(Vec<TestGoal>),
}
```

### 10.1 default preset

- `unit_default = [Transitions, TransitionPairs { k: 2 }, GuardBoundaries]`
- `e2e_default = [PropertyPrefixes, Goals(trace_acceptance)]`
- `concurrency_default = [Transitions, TransitionPairs { k: 2 }]`

ここで `PropertyPrefixes` は temporal property そのものを完全にテストするのではなく、prefix / bounded lasso / bad-prefix を優先して test goal に落とす。これは explicit bounded lasso や trace validation と相性がよい。

## 11. Engine の再設計

`code_tests` は engine を 1 つに固定しない。

```rust
pub enum TestEngine {
    ExplicitSuite,
    ProptestOnline,
    KaniBounded,
    TraceValidation,
    LoomSmall,
    ShuttlePCT,
    // future:
    Lincheck,
    StaterightLinearizability,
}
```

## 11.1 `ExplicitSuite`

explicit reachable graph / bounded lasso から test obligation を抽出し、coverage goal を満たす suite を作る。v1 は shortest-path ベースでよく、将来的には transition-tour / Postman 系の suite compression を追加する。Spec Explorer 系でも explored graph から representative subset を得て test case を作り、Postman tour は古典的な最小リンク被覆の一つである。[^specexplorer_book][^postman]

## 11.2 `ProptestOnline`

`proptest-state-machine` を backend として使う。ただし `nirvash` は `StateMachineTest` 実装をユーザーに書かせない。spec と binding から自動生成する。現状 `proptest-state-machine` は `sequential` のみ正式サポートなので、v1 の online engine は sequential に限定する。[^proptest_sm][^proptest_sm_docs]

## 11.3 `KaniBounded`

Kani 用の proof harness を自動生成する。主用途は 1-step refinement、短い sequence refinement、panic / bounds / invariant の bounded exhaustive check である。failure 時には concrete playback を有効化し、replayable Rust unit test を生成できるようにする。[^kani_playback][^kani_usage]

## 11.4 `TraceValidation`

実装から partial trace を取り、spec 側で constrained model checking する。TLA+ trace validation が示すとおり、full state dump ではなく spec variable updates だけでも十分であり、しかも一部変数だけの logging でも checker 側が補完できる。`nirvash` における e2e default はこの engine を正本にする。[^trace_validation][^trace_validation_slides]

## 11.5 `LoomSmall`

小さい並行ケースに対して exhaustive schedule exploration を行う。`loom` は C11 memory model 下の valid execution を多回実行で網羅し、state reduction も持つ。ただし thread 数や history が増えると厳しいため、`concurrent_small()` と結びつける。[^loom_docs]

## 11.6 `ShuttlePCT`

`loom` より大きなケースに対して randomized schedule / PCT で探索する。`shuttle` 自体が Loom に対する soundness-scalability trade-off を明示しているため、`TrustTier::Heuristic` 相当として扱う。[^shuttle_docs][^shuttle_pct]

## 11.7 将来拡張: `Lincheck` / `StaterightLinearizability`

concurrent object の線形化可能性が主要性質である場合、`lincheck` や `stateright` の `LinearizabilityTester` と接続する。どちらも sequential reference object に対して concurrent history が線形化可能かを検証する流れを持つ。[^lincheck][^stateright_lin]

## 12. GeneratedHarnessPlan

生成物の中心型を導入する。

```rust
pub struct GeneratedHarnessPlan<Spec: FrontendSpec> {
    pub spec_name: &'static str,
    pub profiles: Vec<TestProfile<Spec>>,
    pub replay_dir: PathBuf,
    pub materialize_failures: bool,
}

pub struct TestProfile<Spec: FrontendSpec> {
    pub label: &'static str,
    pub model_instance: ModelInstance,
    pub seeds: SeedProfile,
    pub coverage: Vec<CoverageGoal>,
    pub engines: Vec<TestEngine>,
}
```

installer macro は local test crate 内で `GeneratedHarnessPlan` を評価し、engine ごとに 1 本または少数の `#[test]` を生成する。重要なのは、**個々のケースをコンパイル時に展開しない**ことである。

## 13. Import-first な利用形

正本フローは次のようにする。

```rust
use my_spec::generated::prelude::*;
use my_runtime::MyRuntime;

nirvash::import_generated_tests! {
    spec = MySpec,
    binding = MyRuntime,
}
```

### 13.1 カスタム profile を import して使う場合

```rust
use my_spec::generated::prelude::*;
use my_spec::generated::profiles::{unit_default, e2e_default};
use my_spec::generated::seeds::{small, boundary};
use my_runtime::MyRuntime;

nirvash::import_generated_tests! {
    spec = MySpec,
    binding = MyRuntime,
    profiles = [
        unit_default().with_seeds(small()),
        unit_default().with_seeds(boundary()),
        e2e_default(),
    ],
}
```

### 13.2 さらに短い sugar

spec crate は profile preset を export する。

```rust
use my_spec::generated::prelude::*;
use my_runtime::MyRuntime;

nirvash::import_generated_tests! {
    spec = my_spec::MySpec,
    binding = MyRuntime,
}
```

canonical path は `nirvash::import_generated_tests!` であり、`generated::install::*` は low-level API として残す。

## 14. `code_tests` macro の新しい意味

`#[code_tests(...)]` は spec 側で次を生成する declarative macro にする。

```rust
#[code_tests(
    models = [small, boundary, concurrent_small],
    coverage = [transitions, transition_pairs(2), guard_boundaries],
    engines = [explicit_suite, proptest_online, kani_bounded, trace_validation],
)]
pub struct MySpec;
```

この macro は次を生成する。

1. `generated` module
2. default `CoverageGoal` preset
3. default `SeedProfile` preset
4. installer macro
5. replay schema
6. metadata for `cargo nirvash materialize-tests`

### 14.1 重要な実装方針

この macro は **test case を source code 化しない**。heavy な suite enumeration や randomized strategy 構築は test runtime で行う。

## 15. `cargo nirvash materialize-tests`

生成物の import-first フローを基本にしつつ、デバッグや CI 固定化のために materialization CLI を追加する。

```text
cargo nirvash materialize-tests --spec MySpec --binding MyRuntime --profile boundary
cargo nirvash replay path/to/failure.ndjson
```

### 15.1 用途

- failing case を固定の Rust test として書き出す
- Kani concrete playback と explicit replay を統一フォーマットへ載せる
- flaky ではない deterministic replay を保存する

## 16. Soundness / TrustTier

生成 engine ごとに `TrustTier` を持たせる。

- `ExplicitSuite`: `Exact` または `CertifiedReduction`
- `ProptestOnline`: `Heuristic`
- `KaniBounded`: `BoundedProof`
- `TraceValidation`: `ConstraintChecked`
- `LoomSmall`: `ScheduleExhaustiveWithinBound`
- `ShuttlePCT`: `Heuristic`

これにより、「自動生成されたテスト」であっても、どこまで sound かを結果に載せられる。

## 17. 実装ステップ

### Phase 1: sequential import-first core

- `generated` module を導入
- `RuntimeBinding` derive を導入
- `SeedProfile` / `CoverageGoal` / `GeneratedHarnessPlan` を導入
- `ExplicitSuite`, `ProptestOnline`, `KaniBounded` を接続
- `import_generated_tests!` を導入

### Phase 2: trace validation first-class

- `TraceValidation` engine を正本化
- partial trace logging API を binding 側へ統合
- replay artifact を NDJSON / JSON で標準化

### Phase 3: concurrency engines

- `LoomSmall` / `ShuttlePCT` を generated engine として追加
- `concurrent_small()` seed profile を有効化
- optional `Lincheck` / `StaterightLinearizability` integration

### Phase 4: materialization / CI ergonomics

- `cargo nirvash materialize-tests`
- `cargo nirvash replay`
- failure minimization と replay bundle 出力

## 18. 互換性と移行

### 18.1 削除対象

- 旧 `code_witness_tests`
- 旧 `code_tests` の直接 test 展開モード
- `ActionApplier` / `StateObserver` を正本とする flow

### 18.2 移行パターン

旧:

```rust
#[code_tests(...)]
mod tests {}
```

新:

```rust
use my_spec::generated::prelude::*;
use my_runtime::MyRuntime;

nirvash::import_generated_tests! {
    spec = MySpec,
    binding = MyRuntime,
}
```

### 18.3 自動移行支援

`cargo nirvash fix-code-tests` を用意し、旧 macro から generated installer への機械変換を提供する。

## 19. 欠点

1. 魔法が増える。
2. proc macro / build / runtime の三層になるため、デバッグのレイヤが増える。
3. binding 推論が失敗したときのエラーメッセージが難しくなる。
4. concurrency engine は sequential よりはるかに高価で、profile 制御が不可欠である。
5. `proptest-state-machine` は現時点で sequential only なので、concurrent engine は別経路が必要である。[^proptest_sm]

## 20. 代替案

### 20.1 旧 `code_tests` を維持し、ユーザーに proptest/loom harness を書かせる

不採用。`nirvash` の価値が「refinement model につながるだけ」に留まり、最終的な test synthesis が手作業のまま残る。

### 20.2 すべてを proc macro expansion で生成する

不採用。compile time が重すぎ、生成物が巨大になり、CI も壊れやすい。

### 20.3 e2e は trace validation のみ、unit/concurrency は範囲外にする

不採用。ユーザーは結局 unit/proptest/loom を手で書くことになり、面倒な部分が残る。

### 20.4 concurrency engine も `proptest-state-machine` に寄せる

不採用。`proptest-state-machine` は現時点で `sequential` を前提とするため、並行系は Loom / Shuttle / Lincheck / Stateright 系の別 engine が自然である。[^proptest_sm][^loom_docs][^shuttle_docs]

## 21. 結論

`nirvash` の `code_tests` は、破壊的変更を許容してでも **import-first な test-synthesis compiler** へ再定義すべきである。

ユーザーに test case を書かせるのではなく、spec と binding と少量の seed override だけを書かせ、残りは generated module を import して使う。ベース入力は `FiniteModelDomain` / `Default` / `Arbitrary` / boundary mining から自動推論し、足りない分だけ builder で補う。この形にすると、`nirvash` は「形式 spec を書ける DSL」から、「その spec から unit / concurrency / e2e のテスト群と replay artifact を自動合成する platform」へ進化する。

---

## 参考文献

[^specexplorer_intro]: Spec Explorer introduction. “test cases are automatically generated from a state-oriented model” and include both “test sequences” and “the test oracle.” <https://learn.microsoft.com/en-us/archive/msdn-magazine/2013/december/model-based-testing-an-introduction-to-model-based-testing-and-spec-explorer>

[^specexplorer_otf]: Veanes et al., online/on-the-fly testing with Spec Explorer. <https://www.microsoft.com/en-us/research/wp-content/uploads/2005/01/otf_fse.pdf>

[^specexplorer_book]: Veanes, Campbell, Schulte, Tillmann. *Model-Based Testing of Object-Oriented Reactive Systems with Spec Explorer*. <https://www.microsoft.com/en-us/research/wp-content/uploads/2008/01/bookChapterOnSE.pdf>

[^modelator_docs]: Modelator documentation and repository. Automatic generation of tests from TLA+ models, interfaces for Rust and Go. <https://mbt.informal.systems/docs/modelator.html>, <https://github.com/informalsystems/modelator>

[^trace_validation]: Cirstea, Kuppe, Loillier, Merz. *Validating Traces of Distributed Programs Against TLA+ Specifications*. <https://arxiv.org/abs/2404.16075>

[^trace_validation_slides]: TLA+ 2024 trace validation slides. <https://conf.tlapl.us/2024/MarkusAKuppe-ValidatingSystemExecutionsWithTheTLAPlusTools.pdf>

[^proptest_sm]: `proptest-state-machine` docs. Sequential state machine testing; current runner supports `sequential`. <https://docs.rs/proptest-state-machine>

[^proptest_sm_docs]: Proptest state-machine tutorial. Counterexample discovery and shrinking against an abstract reference state machine. <https://proptest-rs.github.io/proptest/proptest/state-machine.html>

[^loom_docs]: `loom` docs. Exhaustive-ish schedule exploration under the C11 memory model with state reduction. <https://docs.rs/loom/latest/loom/>

[^shuttle_docs]: `shuttle` docs. Randomized concurrency testing, explicitly framed as a soundness–scalability trade-off relative to Loom. <https://docs.rs/shuttle/latest/shuttle/>

[^shuttle_pct]: `shuttle::check_pct`. PCT scheduler API. <https://docs.rs/shuttle/latest/shuttle/fn.check_pct.html>

[^kani_playback]: Kani concrete playback docs. Generates Rust unit tests from failing proof harnesses. <https://model-checking.github.io/kani/reference/experimental/concrete-playback.html>

[^kani_usage]: Kani usage docs for `--concrete-playback`. <https://model-checking.github.io/kani/usage.html>

[^lincheck]: `lincheck` docs. Linearizability testing for concurrent data structures on top of Loom. <https://docs.rs/lincheck>

[^stateright_lin]: `stateright` `LinearizabilityTester` docs. <https://docs.rs/stateright/latest/stateright/semantics/struct.LinearizabilityTester.html>

[^mbt_taxonomy]: Utting, Pretschner, Legeard. *A taxonomy of model-based testing approaches*. <https://mediatum.ub.tum.de/doc/1246357/396788.pdf>

[^coverage_survey]: Briones. *Theories for Model-based Testing: Real-time and Coverage*. Discussion of state and transition coverage. <https://ris.utwente.nl/ws/files/6041491/thesis_Briones.pdf>

[^postman]: Spec Explorer / Spec# notes mentioning Postman tour for minimal link coverage. <https://staff.washington.edu/jon/icfem/specs-icfem.html>

# RFC 0005: `code_tests` を import-first な自動テスト合成基盤として実装する（新 API のみ）

- Status: Draft
- Author: OpenAI / ChatGPT
- Target: `nirvash`, `nirvash-macros`, `nirvash-lower`, `nirvash-check`, `nirvash-backends`, `nirvash-conformance`, `nirvash-proof`, `cargo-nirvash`
- Type: Breaking change / New-API-only
- Created: 2026-03-14

## 0. 要約

本 RFC は、RFC 0004 の方針を実装レベルに落とし込み、`code_tests` を **import-first な test-synthesis compiler** として実装するための具体案を定義する。

本 RFC は **後方互換性を前提にしない**。旧 `code_tests` / `code_witness_tests` / 旧 conformance 入口は対象外とし、**新 API を唯一の public contract** とする。

中心方針は次の 5 点である。

1. `#[code_tests]` は spec 側で **`generated` companion module** を生成する。
2. テスト利用側は **`generated` を import** し、`generated::install::*` が出す installer macro を 1 回呼ぶだけでよい。
3. ユーザーが手で書くのは、原則として **spec・binding・必要最小限の seed/fixture override** だけにする。
4. ベース入力は `FiniteModelDomain`・`Default`・`Arbitrary`・normalized core からの boundary mining で自動生成し、足りないときだけ `seeds!` / builder で補う。
5. 生成対象は個々の `#[test]` 群ではなく、**`GeneratedHarnessPlan`・shared obligations・engine adapter・replay artifact** とする。重い探索は runtime/CLI 側へ逃がす。

新しい正本の利用形は次の 3 手だけである。

```rust
use my_spec::generated::prelude::*;

#[nirvash_binding(spec = my_spec::MySpec)]
impl MyRuntime {
    fn put(&mut self, key: Key, value: Value) -> Result<(), Error> {
        self.sut.put(key, value)
    }

    fn get(&mut self, key: Key) -> Option<Value> {
        self.sut.get(key)
    }
}

nirvash::import_generated_tests! {
    spec = my_spec::MySpec,
    binding = MyRuntime,
}
```

## 1. 背景

`nirvash` は frontend/core/lower/check/backends/conformance/proof へ分割され、`normalized_core()` を正本に寄せつつ、explicit/symbolic checker と conformance / proof export を sibling crate へ分離している。したがって「spec から verifier へ落とす」基盤は整っている。問題は、**spec から実コードの test suite までを完全自動で落とす compile/runtime pipeline** がまだ public contract として固定されていない点にある。

RFC 0004 では `code_tests` を import-first な test compiler へ再定義した。本 RFC はそのうち、実装に必要な部分を固定する。

- どの crate が何を持つか
- どの macro が何を生成するか
- binding surface をどう設計するか
- base input / fixture / environment をどう自動生成するか
- 各 engine を同一 plan にどう接続するか
- materialization / replay / CI をどう扱うか

本 RFC は **旧 API との橋渡しを記述しない**。  
旧 API が必要なら別ブランチで保持すべきであり、ここでは扱わない。

## 2. 目標

### 2.1 主要目標

1. **ゼロ手書き test case** を目標とする。
2. ユーザーは generated artifact を import して使う。
3. 単純な sequential API なら、binding だけで unit suite が立ち上がる。
4. 初期 state、action payload、環境 seed は、最初は自動生成される。
5. explicit / proptest / kani / trace validation / loom / shuttle を同一 spec から合成できる。
6. 失敗時は replay artifact と materialized unit test を自動出力できる。

### 2.2 非目標

1. 任意の外部 I/O を完全自動推論すること。
2. Rust 言語の制約を無視して spec ごとの proc-macro derive を生成すること。
3. 巨大なテストケース群をすべて compile-time に source code 化すること。
4. v1 で async/distributed/concurrency のすべてを完全自動にすること。
5. 旧 API を維持すること。

## 3. 設計原則

### 3.1 import-first

正本は **generated module を import して installer macro を使う** 形にする。  
汎用 installer は置かず、spec ごとに生成される

- `my_spec::generated::prelude::*`
- `my_spec::generated::profiles::*`
- `my_spec::generated::seeds::*`
- `generated::install::*` (`nirvash::import_generated_tests!` が canonical。nested spec module から crate root で直接使う場合は `generated` を re-export してから使う)

を import する flow を正本とする。

### 3.2 compile-time は metadata、runtime は synthesis

proc macro は **metadata / plan / installer** だけを生成し、suite enumeration・search・randomized generation・trace validation・replay bundle 生成は runtime/CLI 側へ逃がす。

### 3.3 engine 共通の obligation IR

各 engine が個別に spec を読むのではなく、まず normalized core から **`TestObligation`** を作る。  
そのうえで explicit suite, proptest, kani, trace validation, loom, shuttle が obligation をそれぞれ消費する。

### 3.4 binding は `impl` attribute を正本にする

Rust では derive macro 単体では method 実装を見られない。  
したがって action-method 対応の自動推論は `impl` block に付く attribute macro で実装する。正本は

```rust
#[nirvash_binding(spec = my_spec::MySpec)]
impl MyRuntime { ... }
```

である。

### 3.5 ベース入力は自動推論が正本

seed override は escape hatch であって、主経路ではない。  
型 domain、spec literal、guard threshold、initial states、environment seed からまず自動生成し、足りないときだけ profile builder と `seeds!` マクロで補う。

## 4. 新 API の public contract

この RFC が定義する public contract は次の 4 つだけである。

1. spec 側の `#[code_tests(...)]`
2. runtime 側の `#[nirvash_binding(spec = ...)] impl ...`
3. generated 側の `generated::{prelude, seeds, profiles, install, replay}`
4. CLI 側の `cargo nirvash {list-tests, materialize-tests, replay}`

旧 `code_tests` / `code_witness_tests` / generic import macro / compatibility shim は public contract に含めない。

## 5. Crate ごとの変更

## 5.1 `nirvash-macros`

追加/変更するマクロ:

- `#[code_tests(...)]`
- `#[nirvash_binding(spec = ...)]` on `impl`
- `#[nirvash_fixture]` on zero-arg constructor or free function
- `#[nirvash_project]` on projection method
- `seeds! { ... }`
- `profiles! { ... }`

### 5.1.1 `#[code_tests]` の責務

1. spec item の横に `pub mod generated` を生成
2. `GeneratedSpecMetadata` 定数を生成
3. default `CoverageGoal` preset を生成
4. default `SeedProfile` preset を生成
5. installer macro を生成
6. replay artifact schema を生成
7. `cargo-nirvash` が読む manifest path を埋め込む

### 5.1.2 `#[nirvash_binding]` の責務

`impl` block から以下を生成する。

- `impl RuntimeBinding<MySpec> for MyRuntime`
- action dispatch table
- projection glue
- fixture factory glue
- trace hook glue
- engine support marker (`SequentialOnly` / `ConcurrentCapable`)

action 名の推論規則は次とする。

1. `#[nirvash(action = Put)]`
2. method 名 `put` -> enum variant `Put`
3. method 名 `exec_put` -> `Put`
4. 失敗時は compile error で action attribute を要求

projection の推論規則は次とする。

1. `#[nirvash_project] fn project(&self) -> SpecState`
2. `SpecState: From<&Self>`
3. `SpecState: From<&Sut>`
4. field 名一致による struct projection（限定サポート）
5. 不明なら compile error

fixture の推論規則は次とする。

1. `#[nirvash_fixture] fn fixture() -> Fixture`
2. `Default`
3. zero-arg `new()`
4. zero-arg `builder().build()`
5. 不明なら compile error

## 5.2 `nirvash-conformance`

新しい public surface:

```rust
pub struct GeneratedHarnessPlan<Spec: FrontendSpec> {
    pub metadata: GeneratedSpecMetadata,
    pub profiles: Vec<TestProfile<Spec>>,
    pub artifact_dir: ArtifactDirPolicy,
}

pub struct GeneratedSpecMetadata {
    pub spec_name: &'static str,
    pub export_module: &'static str,
    pub normalized_fragment: NormalizedFragmentInfo,
    pub default_profiles: &'static [&'static str],
}

pub struct TestProfile<Spec: FrontendSpec> {
    pub label: &'static str,
    pub model_instance: ModelInstance,
    pub seeds: SeedProfile<Spec>,
    pub coverage: Vec<CoverageGoal>,
    pub engines: Vec<EnginePlan>,
}
```

### 5.2.1 新しい trait

```rust
pub trait RuntimeBinding<Spec: FrontendSpec>: Sized {
    type Sut;
    type Fixture: Clone + Send + Sync + 'static;
    type Output: Send + Sync + 'static;
    type Error: std::error::Error + Send + Sync + 'static;

    fn create(fixture: &Self::Fixture) -> Result<Self::Sut, Self::Error>;
    fn reset(sut: &mut Self::Sut, fixture: &Self::Fixture) -> Result<(), Self::Error>;
    fn apply(
        sut: &mut Self::Sut,
        action: &Spec::Action,
        env: &mut TestEnvironment,
    ) -> Result<Self::Output, Self::Error>;
    fn project(sut: &Self::Sut) -> ProjectedState<Spec::State>;
}

pub trait TraceBinding<Spec: FrontendSpec>: RuntimeBinding<Spec> {
    fn record_update(
        sut: &Self::Sut,
        output: &Self::Output,
        sink: &mut dyn TraceSink<Spec>,
    );
}
```

`reset` は既定実装で `create` し直してよい。`TraceBinding` 未実装時は `trace_validation` engine を compile error にする。

### 5.2.2 共通 obligation IR

```rust
pub struct TestObligation<Spec: FrontendSpec> {
    pub id: ObligationId,
    pub kind: ObligationKind<Spec>,
    pub trust_floor: TrustTier,
    pub witness_hint: Option<Trace<Spec::State, Spec::Action>>,
}

pub enum ObligationKind<Spec: FrontendSpec> {
    Transition { action: Spec::Action },
    TransitionPair { prefix: Vec<Spec::Action> },
    GuardBoundary { action: Spec::Action, label: &'static str },
    PropertyPrefix { property: &'static str, depth: usize },
    Goal { label: &'static str },
}
```

### 5.2.3 seed / fixture / environment 型

```rust
pub struct SeedProfile<Spec: FrontendSpec> {
    pub fixture: FixtureSeed,
    pub state: StateSeed<Spec::State>,
    pub actions: ActionSeedMap<Spec::Action>,
    pub environment: EnvironmentSeed,
    pub shrink: ShrinkPolicy,
}

pub enum FixtureSeed {
    Default,
    Factory(fn() -> Box<dyn std::any::Any + Send + Sync>),
    Snapshot(serde_json::Value),
}

pub struct EnvironmentSeed {
    pub rng_seed: u64,
    pub clock_seed: u64,
    pub schedule_seed: u64,
    pub io_mode: IoMode,
    pub network_mode: NetworkMode,
}
```

公開 builder API:

```rust
small()
boundary()
concurrent_small()
e2e_default()
soak()
```

および

```rust
small()
    .with_fixture(MyRuntime::default())
    .with_seed::<Key>(["a", "b", "c"])
    .with_action_seed::<Action::Put>([(Key::from("a"), Value::from(0))])
    .with_rng_seed(7)
```

### 5.2.4 `seeds!` マクロ

ユーザーの override は `seeds!` を正本にする。

```rust
let p = small().with(seeds! {
    fixture = MyRuntime::default();
    type Key = ["a", "b", "c"];
    type Value = [0, 1, 255, 256];
    action Put = [
        (Key::from("a"), Value::from(0)),
        (Key::from("b"), Value::from(255)),
    ];
    rng = 7;
    clock = 42;
});
```

このマクロは `SeedOverrideSet` を返し、profile builder に merge される。

## 5.3 `nirvash-lower`

`LoweredSpec` から `code_tests` が使う surface を明示化する。

追加 API:

```rust
impl<S: FrontendSpec> LoweredSpec<S> {
    pub fn generated_test_core(&self) -> &NormalizedSpecCore;
    pub fn generated_test_domains(&self) -> GeneratedDomainInfo;
    pub fn generated_test_boundaries(&self) -> BoundaryCatalog;
}
```

### 5.3.1 `BoundaryMining` pass

`normalized_core()` から deterministic に boundary 候補を抽出する pass を追加する。

抽出対象:

- `x < C`, `x <= C`, `x == C`, `x >= C`, `x > C`
- sequence / set / map cardinality threshold
- literal enum values
- action subset literals
- update に現れる numeric/string literal
- temporal bad-prefix で現れる action guard

出力:

```rust
pub struct BoundaryCatalog {
    pub by_type: BTreeMap<TypeKey, SeedCandidates>,
    pub by_action: BTreeMap<ActionKey, SeedCandidates>,
}
```

## 5.4 `nirvash-check`

共通 obligation から engine-specific planning を行う planner API を追加する。

```rust
pub trait ObligationPlanner<Spec: FrontendSpec> {
    fn obligations(
        lowered: &LoweredSpec<Spec>,
        model: &ModelInstance,
        coverage: &[CoverageGoal],
        seeds: &SeedProfile<Spec>,
    ) -> Result<Vec<TestObligation<Spec>>, PlannerError>;
}
```

v1 の実装:

- `ExplicitObligationPlanner`
- `PropertyPrefixPlanner`
- `TraceConstraintPlanner`

## 5.5 `nirvash-backends`

### 5.5.1 explicit suite planner

新規モジュール:

- `explicit::suite_planner`
- `explicit::obligation_cover`
- `explicit::transition_tour`（v2）

v1 は shortest-path cover でよい。  
各 obligation に対して minimal witness trace を求め、重複 prefix を共有する。

### 5.5.2 symbolic trace validation helper

新規モジュール:

- `symbolic::trace_constraints`

`TraceValidation` engine のために observed updates から constrained model checking problem を構築する。

## 5.6 `cargo-nirvash`

新規サブコマンド:

- `cargo nirvash list-tests`
- `cargo nirvash materialize-tests`
- `cargo nirvash replay`

互換用 `fix-code-tests` は置かない。

### 5.6.1 `materialize-tests`

失敗ケースや selected profile から、固定 Rust tests を materialize する。

```text
cargo nirvash materialize-tests --spec my_spec::MySpec --binding MyRuntime --profile boundary
```

出力:

- `tests/generated/<spec>_<profile>_replay.rs`
- `target/nirvash/replay/<run-id>.ndjson`
- `target/nirvash/replay/<run-id>.json`

## 6. 生成される `generated` module 契約

`#[code_tests]` は次を生成する。

```rust
pub mod generated {
    pub mod prelude;
    pub mod metadata;
    pub mod seeds;
    pub mod profiles;
    pub mod plans;
    pub mod install;
    pub mod replay;
    pub mod bindings;
}
```

### 6.1 `generated::prelude`

`prelude` は以下を再 export する。

- `nirvash_binding`
- `seeds!`
- `profiles!`
- `small`, `boundary`, `concurrent_small`, `e2e_default`, `soak`
- `CoverageGoal`
- `TrustTier`
- spec 固有の installer macro 群

これにより、ユーザーは `use my_spec::generated::prelude::*;` だけで始められる。

### 6.2 `generated::profiles`

最低限生成する preset:

- `unit_default()`
- `boundary_default()`
- `e2e_default()`
- `concurrency_default()`
- `smoke_default()`

実体は `TestProfileBuilder`。

### 6.3 `generated::install`

installer macro 群:

```rust
all_tests!(binding = MyRuntime);
tests!(binding = MyRuntime, profiles = [unit_default(), e2e_default()]);
unit_tests!(binding = MyRuntime);
trace_tests!(binding = MyRuntime);
kani_harnesses!(binding = MyRuntime); // cfg(kani)
loom_tests!(binding = MyRuntime);     // feature = "loom"
```

重要なのは、これらが **少数の harness** しか生やさないことである。  
個々の test case は runtime で `GeneratedHarnessPlan` から評価される。

## 7. `#[code_tests]` の新 grammar

```rust
#[code_tests(
    export = generated,
    models = [small, boundary, concurrent_small, e2e_default],
    profiles = [
        smoke_default = {
            coverage = [transitions],
            engines = [explicit_suite],
        },
        unit_default = {
            coverage = [transitions, transition_pairs(2), guard_boundaries],
            engines = [explicit_suite, proptest_online(cases = 4096, steps = 1..32), kani(depth = 4)],
        },
        e2e_default = {
            coverage = [property_prefixes],
            engines = [trace_validation],
        },
        concurrency_default = {
            coverage = [transitions, transition_pairs(2)],
            engines = [loom_small(threads = 2), shuttle_pct(depth = 2, runs = 2000)],
        },
    ],
)]
pub struct MySpec;
```

### 7.1 v1 の省略規則

`profiles` 未指定時は次を自動生成する。

- `smoke_default`
- `unit_default`
- `e2e_default`

`models` 未指定時は `small`, `boundary`, `e2e_default` を使う。

## 8. ユーザーが書くコード

## 8.1 最小形

```rust
use my_spec::generated::prelude::*;

pub struct MyRuntime {
    sut: MyService,
}

#[nirvash_binding(spec = my_spec::MySpec)]
impl MyRuntime {
    fn put(&mut self, key: Key, value: Value) -> Result<(), Error> {
        self.sut.put(key, value)
    }

    fn get(&mut self, key: Key) -> Option<Value> {
        self.sut.get(key)
    }
}

nirvash::import_generated_tests! {
    spec = my_spec::MySpec,
    binding = MyRuntime,
}
```

この場合、fixture は `Default` / `new()` 推論、seed は finite domain + boundary mining 推論を使う。

## 8.2 ほんの少しだけ override する形

```rust
use my_spec::generated::prelude::*;

pub struct MyRuntime {
    sut: MyService,
}

#[nirvash_binding(spec = my_spec::MySpec)]
impl MyRuntime {
    #[nirvash_fixture]
    fn fixture() -> Self {
        Self { sut: MyService::with_capacity(16) }
    }

    fn put(&mut self, key: Key, value: Value) -> Result<(), Error> {
        self.sut.put(key, value)
    }

    fn get(&mut self, key: Key) -> Option<Value> {
        self.sut.get(key)
    }
}

nirvash::import_generated_tests! {
    spec = my_spec::MySpec,
    binding = MyRuntime,
    profiles = [
        unit_default().with(seeds! {
            type Key = ["a", "b", "c"];
            type Value = [0, 1, 15, 16];
        }),
        e2e_default().with_rng_seed(7),
    ],
}
```

## 8.3 projection が必要な場合

```rust
use my_spec::generated::prelude::*;

pub struct MyRuntime {
    sut: MyService,
}

#[nirvash_binding(spec = my_spec::MySpec)]
impl MyRuntime {
    fn put(&mut self, key: Key, value: Value) -> Result<(), Error> {
        self.sut.put(key, value)
    }

    #[nirvash_project]
    fn project(&self) -> my_spec::State {
        self.sut.snapshot().into_spec_state()
    }
}

nirvash::import_generated_tests! {
    spec = my_spec::MySpec,
    binding = MyRuntime,
    profiles = [unit_default()],
}
```

## 9. engine 実装の正本

## 9.1 `ExplicitSuite`

- planner: `ExplicitObligationPlanner`
- obligation source: explicit reachable graph
- artifact: `Exact` / `CertifiedReduction` / `ClaimedReduction`
- default use: `smoke_default`, `unit_default`

v1 アルゴリズム:
1. obligations を列挙
2. 各 obligation の shortest witness trace を求める
3. prefix-sharing で suite を圧縮
4. replay bundle を生成

## 9.2 `ProptestOnline`

- planner input: obligations + seed profile
- generator: auto-generated `StateMachineAdapter`
- restriction: v1 は sequential only
- verdict: `Heuristic`

v1 実装では `TransitionPairs(k)` と `GuardBoundaries` を重み付けに使う。  
covered でない obligation に重みを寄せる。

## 9.3 `KaniBounded`

- planner input: obligations から bounded proof harness 候補を作る
- output: `cfg(kani)` 下の proof harness module
- failure artifact: Kani concrete playback と統合

installer macro は通常 `#[test]` を生やさず、`cfg(kani)` のときだけ `#[kani::proof]` を生成する。

## 9.4 `TraceValidation`

- input: `TraceBinding` が記録した observed updates
- checker: explicit or symbolic constrained checking
- default use: `e2e_default`

artifact 形式:

```rust
pub enum ObservedEvent<Action, Output> {
    Invoke { action: Action },
    Return { action: Action, output: Option<Output> },
    Update { var: &'static str, value: serde_json::Value },
    Stutter,
}
```

v1 では `Update` と `Invoke/Return` のみ正式サポート。  
`Internal` event は v2 とする。

## 9.5 `LoomSmall`

- feature gated
- `concurrent_small()` を必須
- default parameters: threads=2, max_permutations=config default
- verdict: `ScheduleExhaustiveWithinBound`

`RuntimeBinding` に加えて `ConcurrentBinding` marker が必要。  
marker が無い場合 installer は compile error で `concurrency_default()` を拒否する。

## 9.6 `ShuttlePCT`

- feature gated
- `concurrent_small()` または明示 profile
- verdict: `Heuristic`
- replay: encoded schedule + seed

## 10. ベース入力の自動生成

## 10.1 自動生成優先順位

各型 `T` に対し seed 候補を次の順で集め、stable dedup する。

1. `FiniteModelDomain<T>`
2. `BoundaryCatalog` の `T`
3. `Arbitrary<T>` / registered `Strategy<T>`
4. `Default<T>`
5. singleton seed (`0`, `""`, empty collection, first enum variant) の built-in fallback

同じく action payload は

1. action 固有 seed
2. payload 型 seed の直積
3. guard-aware pruning
4. invalid 組の除去

の順で作る。

## 10.2 initial state / fixture の自動生成

fixture は以下の順で決める。

1. `#[nirvash_fixture]`
2. `Default`
3. `new()`
4. `builder().build()`
5. snapshot JSON replay
6. compile error

spec の初期状態が複数ある場合は、

- projection がそれに一致する fixture を優先
- 一致しない fixture は `InitMismatch` として fail-fast
- `with_initial_state(...)` override で特定初期状態を選べる

## 10.3 シンプル override API

基本 builder API に加え、簡単な sugar を用意する。

```rust
small_keys(["a", "b", "c"])
boundary_numbers::<u64>()
smoke_fixture(MyRuntime::default())
```

これらは `SeedOverrideSet` を返し、`with(...)` に渡せる。

## 11. 生成 artifact と replay

標準保存先:

- `target/nirvash/manifest/<spec>.json`
- `target/nirvash/replay/<run-id>.ndjson`
- `target/nirvash/replay/<run-id>.json`
- `tests/generated/<spec>_<profile>_replay.rs`
- `tests/generated/<spec>_<profile>_kani.rs`

### 11.1 NDJSON schema

各行は 1 event。

```json
{"kind":"invoke","action":"Put","args":["a",1]}
{"kind":"update","var":"store","value":{"a":1}}
{"kind":"return","action":"Put","ok":true}
```

### 11.2 replay import

generated module は replay helper を export する。

```rust
my_spec::generated::replay::run::<MyRuntime>("target/nirvash/replay/fail.ndjson")?;
```

## 12. 実装順序

## Phase 1: import-first sequential core

対象:
- `#[code_tests]`
- `#[nirvash_binding]`
- `GeneratedHarnessPlan`
- `SeedProfile`
- `CoverageGoal`
- `ExplicitSuite`
- `ProptestOnline`
- generated installer macro

Done 条件:
1. 最小例が test case 手書きなしで動く
2. `small()` だけで explicit/proptest suite が立つ
3. failure で replay bundle が出る

## Phase 2: Kani / materialization / replay

対象:
- `KaniBounded`
- `cargo nirvash materialize-tests`
- `cargo nirvash replay`
- concrete playback 統合

Done 条件:
1. `cfg(kani)` で proof harness が生成される
2. failing harness から replay unit test を materialize できる

## Phase 3: trace validation

対象:
- `TraceBinding`
- observed update schema
- constrained checker bridge
- `trace_tests!`

Done 条件:
1. partial trace だけで e2e validation が動く
2. full state dump を要求しない

## Phase 4: concurrency

対象:
- `LoomSmall`
- `ShuttlePCT`
- `ConcurrentBinding`
- `concurrent_small()`

Done 条件:
1. 2-thread small concurrency が自動起動する
2. replayable schedule artifact が保存される

## 13. 受け入れ条件

本 RFC の実装は、少なくとも次を満たしたとき完了とみなす。

1. **import-first**  
   generated module を import して 1 行の installer macro を呼ぶだけで test 群が起動する。

2. **最小ユーザー記述**  
   単純な CRUD spec では binding だけで explicit/proptest suite が動く。

3. **seed 自動生成**  
   finite domain + boundary mining + default fixture だけで初回実行が通る。

4. **runtime replay**  
   failure で NDJSON replay bundle が保存される。

5. **materialization**  
   CLI で replay bundle から固定 test を materialize できる。

6. **kani integration**  
   `cfg(kani)` 下で proof harness が生成される。

7. **old-surface-free**  
   旧 `code_tests` / `code_witness_tests` / compatibility shim を public API に残さない。

## 14. 欠点

1. public surface が増える。
2. macro error が複雑になる。
3. seed 自動生成は万能ではない。
4. compile-time metadata と runtime synthesis の二層になる。
5. v1 の online engine は sequential のみに限定される。

## 15. 代替案

### 15.1 derive macro だけで binding を解決する

不採用。derive は impl method を見られないため、自動 action dispatch 生成が弱い。

### 15.2 すべての test case を compile-time で展開する

不採用。コンパイル時間と生成コード量が破綻しやすい。

### 15.3 generic installer を正本にする

不採用。import-first の利点が薄れ、spec 固有の generated helper が活きない。

### 15.4 compatibility shim を残す

不採用。今回の前提は新 API のみであり、複数 surface を持つこと自体が実装負債になる。

## 16. 結論

本 RFC は、`code_tests` を実装可能な形で **generated-module 中心、binding impl-attribute 中心、seed 自動生成中心** に固定する。これにより、`nirvash` は「spec へ checker をつなぐ基盤」から、「spec から unit / bounded proof / e2e validation / concurrency harness まで自動合成する基盤」へ進む。

正本の UX は次の 3 手だけである。

1. spec に `#[code_tests]`
2. runtime に `#[nirvash_binding(spec = ...)] impl ...`
3. test module で `nirvash::import_generated_tests! { spec = my_spec::MySpec, binding = ... }`

これ以上の記述は override であり、主経路ではない。

---

## 参考文献

- Spec Explorer introduction: model から test sequence と oracle を自動生成する MBT ツール。
- Spec Explorer on-the-fly testing: test derivation と test execution の統合。
- `proptest-state-machine`: sequential state machine testing。
- `loom`: C11 memory model 下での schedule permutation exploration。
- `shuttle`: randomized / PCT / DFS scheduler。
- Kani concrete playback: failing harness から Rust unit test を生成。
- TLA+ trace validation: observed updates を使う constrained model checking。
