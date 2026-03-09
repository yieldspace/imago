# imagod-spec

Rust DSL ベースで `imagod` 全体の system spec を表現する crate です。  
現行の top-level 主説明は `system` で、単一 `imagod` daemon の boot / session / wire / deploy / supervision / RPC / plugin / shutdown を relation-first に束ねます。旧 synchronized baseline は `legacy_system` として残しています。

## DSL

- 通常 spec の正本は `TransitionSystem::initial_states()`、`TransitionSystem::actions()`、`TransitionSystem::transition()` です。並行 spec は `ConcurrentTransitionSystem::{initial_states, atomic_actions, atomic_transition, footprint_reads, footprint_writes}` を atomic 正本にし、top-level `TransitionSystem::Action = ConcurrentAction<_>` と `successors()` で synthesized parallel step を返します。`CommandProtocolState`、`ManagerRuntimeState`、`SessionTransportState` のような spec-local state 自体には原則 `Signature` を付けません。
- `#[derive(Signature)]` は helper enum/newtype、projection 型、bounded data 用に使います。
- 構造を持つ state は relation-first を既定にし、helper atom に `#[derive(RelAtom)]`、state には `RelSet<T>` / `Relation2<A, B>` と `#[derive(RelationalState)]` を使います。phase progression や terminal status のような線形 workflow gate だけ scalar enum/bool を維持します。
- 推奨順は 3 段階です。
  - まず helper 型の auto derive に任せる
  - 次に `#[signature(bounds(...))]` と `#[signature(filter(self => ...))]`、必要なら `#[signature_invariant(self => ...)]` で bounded helper domain を絞る
  - それでも表現しきれない型だけ `#[signature(custom)]` にして companion trait を手書きするか、`nirvash_core::signature_spec!(StateSignatureSpec for State, ...)` で sugar を使う
- field ごとの bounded domain は `#[sig(range = "...")]`、`#[sig(len = "A..=B")]`、`#[sig(domain = path)]` で上書きできます。`Option<T>` はデフォルトで `None + T::bounded_domain()` を使います。
- subsystem spec は `#[subsystem_spec(...)]`、top-level system spec は `#[system_spec(...)]` で `TemporalSpec` を自動生成します。
- `#[invariant(SpecType)]`、`#[property(SpecType)]`、`#[fairness(SpecType)]`、`#[state_constraint(SpecType)]` などの target-spec 付き attribute が registry へ自動登録され、`TemporalSpec` と `ModelCase` から自動収集されます。
- 加算 DSL として `nirvash_core::invariant!(Spec, name(state) => ...)`、`nirvash_core::property!(Spec, name => leads_to(...))`、`nirvash_core::fairness!(weak Spec, ...)` などの `macro_rules!` 宣言も使えます。
- `ltl!` 内では Rust に既にある `!` / `&&` / `||` / `=>` を使い、時相演算だけ `always` / `eventually` / `next` / `until` / `enabled` / `leads_to` の単語で補います。
- `#[formal_tests(...)]` が初期状態 invariant / reachable graph checker / composition regression test を自動生成します。
- `#[code_tests(...)]` は `nirvash_core::conformance::ProtocolConformanceSpec` と `ProtocolRuntimeBinding` を使って grouped な runtime conformance を自動生成します。`transition(state, action)` が allowed/rejected の正本で、`expected_output(state, action, next)` が output 契約の正本です。現行の runtime binding は `system` を境界ごとに投影して接続しており、`command_projection -> imagod-control/tests/command_protocol_conformance.rs`、`router_projection` / `session_auth_projection` / `logs_projection -> imagod-server` の source unit test、`runtime_projection -> imagod-control/src/service_supervisor.rs`、`manager_runtime_projection -> imagod/src/manager_runtime.rs` で実行します。
- `#[code_witness_tests(...)]` は `ProtocolInputWitnessBinding` で concrete input witness を供給し、reachable graph から抽出した semantic case を witness 単位の個別 test として実行します。formal の `model_cases` は探索分割用に維持したまま、runtime conformance では binding 実装者が `model_case` を意識する必要はありません。
- `checker_config(...)` と `cases = model_cases` に加え、`#[state_constraint(SpecType)]`、`#[action_constraint(SpecType)]`、`#[symmetry(SpecType)]` で TLC 相当の model control を Rust API で与えます。
- `build.rs` で `nirvash_docgen::generate()` を呼んでいるため、`cargo doc -p imagod-spec` では各 spec type の `TransitionSystem` impl section に reachable graph 由来の Mermaid `State Graph` と、登録関数一覧を持つ Mermaid `Meta Model` 図が自動表示されます。relational state には追加で `Relation Schema` section が出て、`RelSet` / `Relation2` field が Alloy 風 notation で legend に出ます。`State Graph` は docs 専用の boundary-path reduction を通すため、通常経路の中間 state は折り畳まれ、同じ始点/終点に向かう平行 edge も 1 本にまとめられます。concurrent spec の composite edge は `parallel(a, b, c)` として表示されます。Mermaid runtime は doc fragment に inline で埋め込まれるため、`cargo doc --open` でも `file://` の local asset 読み込みに依存しません。

## TLA+ Subset

- `[]`, `<>`, `X`, `U`, `ENABLED`, `~>` を Rust DSL として使います。
- checker は reachable graph を既定にし、internal stuttering step を含む lasso trace 上で時相性質を評価します。
- fairness は `Fairness::weak(...)` と `Fairness::strong(...)` で表し、generated tests は公平性を前提に liveness を検証します。
- quantifier は `nirvash-core` の `Ltl::forall` / `Ltl::exists` で bounded domain へ展開します。
- deadlock check、state/action constraint、model case、opaque model value、symmetry reduction を Rust DSL で表現できます。

## Bounds

- services <= 2
- sessions <= 2
- runners <= 2
- artifact_chunks <= 2
- plugin_deps <= 3
- http_queue_depth <= 2
- epoch_ticks <= 3
- time_steps <= 4

## Coverage Matrix

| Spec module | Production scope |
| --- | --- |
| `manager_runtime` | `crates/imagod/src/manager_runtime.rs`, `crates/imagod-config` |
| `session_transport` | `crates/imagod-server/src/protocol_handler.rs`, `crates/imagod-server/src/transport` |
| `session_auth` | `crates/imagod-server/src/protocol_handler.rs`, `crates/imagod-server/src/session` |
| `session_auth_projection` | `system` から session auth boundary を投影する spec。`imagod-server` binding 用 |
| `wire_protocol` | `crates/imagod-server/src/protocol_handler/router.rs`, `crates/imagod-server/src/transport` |
| `router_projection` | `system` から router request/response surface を投影する spec。`imagod-server` binding 用 |
| `logs_projection` | `system` から logs ack/chunk/end surface を投影する spec。`imagod-server` binding 用 |
| `command_protocol` | `crates/imago-protocol/src/command_contract.rs`, `crates/imagod-control/src/operation_state.rs` |
| `command_projection` | `crates/imagod-control/tests/command_protocol_conformance.rs` で `system` から command surface を投影 |
| `runtime_projection` | `crates/imagod-control/src/service_supervisor.rs` で deploy / supervision / rpc / shutdown の runtime surface を `system` から投影 |
| `manager_runtime_projection` | `crates/imagod/src/manager_runtime.rs` で boot / maintenance / shutdown milestone を `system` から投影 |
| `artifact_deploy` | Legacy deploy baseline |
| `deploy` | `crates/imagod-control/src/artifact_store.rs`, `crates/imagod-control/src/orchestrator.rs` |
| `supervision` | `crates/imagod-control/src/service_supervisor.rs`, `crates/imagod-runtime-bootstrap` |
| `rpc` | `crates/imagod-control/src/service_supervisor.rs`, `crates/imagod-server/src/protocol_handler.rs` |
| `service_supervision` | Legacy supervision baseline |
| `runner_bootstrap` | Supplemental bootstrap submodel |
| `runner_runtime` | Supplemental runner runtime submodel |
| `plugin_platform` | `crates/imagod-runtime-wasmtime/src/plugin_resolver.rs`, `crates/imagod-runtime-wasmtime/src/capability_checker.rs` |
| `shutdown_flow` | `crates/imagod/src/manager_runtime.rs`, `crates/imagod/src/shutdown.rs` |
| `system` | Unified top-level source of truth for daemon-visible boot / session / wire / deploy / supervision / RPC / plugin / shutdown contracts |
| `legacy_system` | Legacy synchronized composition baseline |

## Defaults

- 1 observable event = 1 temporal step
- public contracts only; no private implementation state
- native plugin internal device logic is out of scope
- model checking は reachable graph semantics を既定とし、必要時だけ bounded lasso mode を使う
- `Signature` は bounded helper data に限定し、state space の正本には使わない
- `imago-protocol::command_contract` と `imagod-control` の command contract は常設し、release 時の負荷は runtime state を最小に保つことで抑える
- runtime conformance の正本は `system` で、boundary ごとに `*_projection` spec へ射影する。`command_projection` は witness-based に `OperationManager` へ接続し、`router_projection` / `session_auth_projection` / `logs_projection` は `imagod-server`、`runtime_projection` は `imagod-control`、`manager_runtime_projection` は `imagod` に grouped `code_tests` で接続する
- `plugin_platform`、`session_auth`、`wire_protocol`、`runner_runtime` は membership / authorization / envelope / capability 関係を relation-first に持ち、cross-link 用の読みやすさは helper predicate で補う
- `system` は runtime private state の完全複製ではなく、daemon-visible contract を resource footprint ベースの synthesized concurrency で束ねる unified top-level の正本として保つ
- `system` の top-level では cross-link invariant と代表経路 reachability を扱い、各 subsystem が既に持つ liveness は二重定義しない
- `legacy_system` は単一 service 前提の旧 baseline として残し、新規説明や docs は `system` を優先する
