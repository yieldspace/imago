# imagod-spec

Rust DSL ベースで `imagod` 全体の system spec を表現する crate です。  
`initial_states + actions + transition` を正本にした reachable graph、LTL/TLA+ practical subset、generated tests を使って subsystem ごとの仕様を自己検証します。

## DSL

- spec state の正本は `TransitionSystem::initial_states()`、`TransitionSystem::actions()`、`TransitionSystem::transition()` です。`successors()` はそこから導出されます。`CommandProtocolState`、`ManagerShellState`、`SessionTransportState` のような spec-local state 自体には原則 `Signature` を付けません。
- `#[derive(Signature)]` は helper enum/newtype、projection 型、bounded data 用に使います。
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
- `#[code_tests(...)]` は `nirvash_core::conformance::ProtocolConformanceSpec` と `ProtocolRuntimeBinding` を使って runtime conformance を自動生成します。`transition(state, action)` が allowed/rejected の正本で、`expected_output(state, action, next)` が output 契約の正本です。`command_protocol` では spec 本体は `imagod-spec` に残し、実行は `imagod-control` の integration test 側で行います。
- `checker_config(...)` と `cases = model_cases` に加え、`#[state_constraint(SpecType)]`、`#[action_constraint(SpecType)]`、`#[symmetry(SpecType)]` で TLC 相当の model control を Rust API で与えます。
- `build.rs` で `nirvash_docgen::generate()` を呼んでいるため、`cargo doc -p imagod-spec` では各 spec type の `TransitionSystem` impl section に reachable graph 由来の Mermaid `State Graph` と、登録関数一覧を持つ Mermaid `Meta Model` 図が自動表示されます。`State Graph` は docs 専用の boundary-path reduction を通すため、通常経路の中間 state は折り畳まれ、同じ始点/終点に向かう平行 edge も 1 本にまとめられます。分岐/合流/終端/cancel などの edge case が優先的に残ります。Mermaid runtime は doc fragment に inline で埋め込まれるため、`cargo doc --open` でも `file://` の local asset 読み込みに依存しません。

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
| `manager_shell` | `crates/imagod/src/manager_runtime.rs`, `crates/imagod-config` |
| `session_transport` | `crates/imagod-server/src/protocol_handler.rs`, `crates/imagod-server/src/transport` |
| `command_protocol` | `crates/imago-protocol/src/command_contract.rs`, `crates/imagod-control/src/operation_state.rs` |
| `artifact_deploy` | `crates/imagod-control/src/artifact_store.rs`, `crates/imagod-control/src/orchestrator.rs` |
| `service_supervision` | `crates/imagod-control/src/service_supervisor.rs` |
| `runner_bootstrap` | `crates/imagod-runtime-bootstrap`, `crates/imagod-ipc::RunnerBootstrap` |
| `runner_runtime` | `crates/imagod-runtime-internal`, `crates/imagod-runtime-wasmtime` |
| `plugin_capability` | `crates/imagod-runtime-wasmtime/src/plugin_resolver.rs`, `crates/imagod-runtime-wasmtime/src/capability_checker.rs` |
| `shutdown_flow` | `crates/imagod/src/manager_runtime.rs`, `crates/imagod/src/shutdown.rs` |
| `system` | Representative synchronized composition across the modules above |

## Defaults

- 1 observable event = 1 temporal step
- public contracts only; no private implementation state
- native plugin internal device logic is out of scope
- model checking は reachable graph semantics を既定とし、必要時だけ bounded lasso mode を使う
- `Signature` は bounded helper data に限定し、state space の正本には使わない
- `imago-protocol::command_contract` と `imagod-control` の command contract は常設し、release 時の負荷は runtime state を最小に保つことで抑える
- `command_protocol` の code conformance は現在 `OperationManager` binding のみに接続され、router の request validation / event sequencing は別境界として扱う
- `plugin_capability` は self-provider と explicit dependency fallback の両方を formal state として持つ
- `system` は exhaustive な全体実装複製ではなく、subsystem transition を同期させる scenario-focused な代表合成モデルとして保つ
- `system` の top-level では cross-link invariant と代表経路 reachability を扱い、各 subsystem が既に持つ liveness は二重定義しない
