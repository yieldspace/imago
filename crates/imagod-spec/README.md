# imagod-spec

Rust DSL ベースで `imagod` 全体の system spec を表現する crate です。  
bounded な state/action universe、LTL/TLA+ practical subset、generated tests を使って
subsystem ごとの仕様を自己検証します。

## DSL

- 値ドメインは `#[derive(Signature)]` で定義します。
- complex state は `#[derive(Signature)] #[signature(custom)]` を付け、生成された companion trait に `representatives()` と必要なら `signature_invariant()` を実装します。
- subsystem spec は `#[subsystem_spec(...)]`、top-level system spec は `#[system_spec(...)]` で `TemporalSpec` を自動生成します。
- `#[invariant(SpecType)]`、`#[illegal(SpecType)]`、`#[property(SpecType)]` などの target-spec 付き attribute が registry へ自動登録され、`TemporalSpec` から自動収集されます。
- 加算 DSL として `nirvash_core::invariant!(Spec, name(state) => ...)`、`nirvash_core::property!(Spec, name => leads_to(...))`、`nirvash_core::fairness!(weak Spec, ...)` などの `macro_rules!` 宣言も使えます。
- `ltl!` 内では Rust に既にある `!` / `&&` / `||` / `=>` を使い、時相演算だけ `always` / `eventually` / `next` / `until` / `enabled` / `leads_to` の単語で補います。
- `#[formal_tests(...)]` が init invariant / reachable graph checker / composition regression test を自動生成します。
- `checker_config(...)` と `cases = model_cases` に加え、`#[state_constraint(SpecType)]`、`#[action_constraint(SpecType)]`、`#[symmetry(SpecType)]` で TLC 相当の model control を Rust API で与えます。
- `build.rs` で `nirvash_docgen::generate()` を呼んでいるため、`cargo doc -p imagod-spec` では各 spec type の `TransitionSystem` impl section に reachable graph 由来の Mermaid `State Graph` と、登録関数一覧を持つ Mermaid `Meta Model` 図が自動表示されます。Mermaid runtime は local asset として docs 出力に同梱されます。

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
| `command_protocol` | `crates/imago-protocol/src/messages/command.rs`, `crates/imagod-control/src/operation_state.rs` |
| `artifact_deploy` | `crates/imagod-control/src/artifact_store.rs`, `crates/imagod-control/src/orchestrator.rs` |
| `service_supervision` | `crates/imagod-control/src/service_supervisor.rs` |
| `runner_bootstrap` | `crates/imagod-runtime-bootstrap`, `crates/imagod-ipc::RunnerBootstrap` |
| `runner_runtime` | `crates/imagod-runtime-internal`, `crates/imagod-runtime-wasmtime` |
| `plugin_capability` | `crates/imagod-runtime-wasmtime/src/plugin_resolver.rs`, `crates/imagod-runtime-wasmtime/src/capability_checker.rs` |
| `shutdown_flow` | `crates/imagod/src/manager_runtime.rs`, `crates/imagod/src/shutdown.rs` |
| `system` | Top-level composition across the modules above |

## Defaults

- 1 observable event = 1 temporal step
- public contracts only; no private implementation state
- native plugin internal device logic is out of scope
- model checking は reachable graph semantics を既定とし、必要時だけ bounded lasso mode を使う
