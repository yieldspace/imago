# imagod Internal Architecture Reference

この文書は `imagod` の内部実装を、実装者と運用者が同じ前提で追える粒度で記述する。

- 対象コード: `crates/imagod/src/*.rs`
- 概要仕様: [`imagod.md`](./imagod.md)
- 関連仕様: [`deploy-protocol.md`](./deploy-protocol.md), [`observability.md`](./observability.md), [`imago-protocol.md`](./imago-protocol.md)

## 1. Scope / 読み方

対象読者: 実装者, 運用者

- 本文書は現行コードの責務分割とデータフローを示す。
- 仕様意図ではなく、どのモジュールが何を処理するかを主軸にする。
- コード断片の引用は最小限とし、`ファイル + 関数名` 参照で追跡する。

対象外:

- restart policy の高度化
- 再起動跨ぎの service 復元
- event 永続化/再送

## 2. プロセス起動とランタイム初期化

対象読者: 実装者, 運用者

実行起点は `crates/imagod/src/main.rs` の `main` / `dispatch`。

初期化順序:

1. `install_rustls_provider`
2. CLI 解析（`manager` / `--runner`）
3. manager モード:
4. `resolve_config_path` + `ImagodConfig::load`
5. `ArtifactStore::new`
6. `OperationManager::new`
7. `ServiceSupervisor::new`（manager control socket 起動）
8. `Orchestrator::new`
9. `ProtocolHandler::new`
10. maintenance loop 起動
11. `build_server` で WebTransport サーバ構築
12. `accept` ループで session task を `tokio::spawn`
13. runner モード:
14. stdin から `RunnerBootstrap` を読込
15. `WasmRuntime::new` + component 実行

```mermaid
flowchart TD
  A["main"] --> B["dispatch"]
  B --> C["install_rustls_provider"]
  B --> D{"mode"}
  D -->|manager| E["ImagodConfig::load"]
  E --> F["ArtifactStore"]
  E --> G["ServiceSupervisor"]
  F --> H["Orchestrator"]
  G --> H
  H --> I["ProtocolHandler"]
  I --> J["maintenance loop"]
  I --> K["WebTransport server"]
  K --> L["session task"]
  D -->|runner| M["RunnerBootstrap(from stdin)"]
  M --> N["WasmRuntime::new + run component"]
```

## 3. モジュール責務マップ

対象読者: 実装者

| モジュール | 主責務 | 主な入力 | 主な出力 | 依存方向 |
|---|---|---|---|---|
| `config.rs` | `imagod.toml` 読込・検証 | 設定パス | `ImagodConfig` | `error.rs`, `imago-protocol` |
| `transport.rs` | mTLS + QUIC/WebTransport endpoint 構築（0-RTT 無効） | TLS 設定, listen_addr | `web_transport_quinn::Server` | `config.rs`, `error.rs` |
| `protocol_handler.rs` | `ProtocolEnvelope<Value>` dispatch | bi-stream bytes | response envelope / command.event | `artifact_store`, `orchestrator`, `operation_state` |
| `artifact_store.rs` | upload session 管理、chunk commit、GC | prepare/push/commit | prepare/ack/commit response | `error.rs` |
| `orchestrator.rs` | deploy/run/stop の実行調停 | command payload | summary / error | `artifact_store`, `service_supervisor` |
| `service_supervisor.rs` | runner child process 監督、control plane | `ServiceLaunch` | start/stop/replace/reap | `ipc`, `runner_process` |
| `runner_process.rs` | runner モード実行（bootstrap, heartbeat, invoke受信） | `RunnerBootstrap` | run result / inbound response | `runtime_wasmtime`, `ipc` |
| `runtime_wasmtime.rs` | runner 内 Wasmtime component 実行 | release path + env + shutdown | `Result<()>` | `error.rs` |
| `ipc/*` | manager-runner/runner-runner IPC 抽象 + 実装 | control/invoke message | response/token | `error.rs` |
| `operation_state.rs` | 短命 operation 状態管理 | UUID + state | `StateResponse`, cancel 判定 | `error.rs` |
| `error.rs` | 内部エラーの構造化 | stage, message, code | `StructuredError` | `imago-protocol` |
| `main.rs` | wiring と maintenance 制御 | config | process lifecycle | 全モジュール |

## 4. 通信処理モデル

対象読者: 実装者, 運用者

通信入口は `crates/imagod/src/protocol_handler.rs` の `handle_session`。

処理モデル:

- session ごとに `accept_bi` ループ。
- stream 受信は `read_to_end` を 30 秒 timeout 付きで実行し、timeout 時は `E_OPERATION_TIMEOUT` で stream を閉じる。
- stream 受信バイトは `decode_frames` でフレーム分解し、各 frame を `from_cbor::<ProtocolEnvelope<Value>>` で復号。
- request envelope は 1 stream につき 1 件のみ許可。複数 request は `E_BAD_REQUEST`。
- `MessageType::CommandStart` は `handle_command_start` へ分岐し、同一 stream へ `command.start response` + `command.event*` を連続送信。
- それ以外は `handle_single` で 1 request -> 1 response。

```mermaid
sequenceDiagram
  participant C as client
  participant S as handle_session
  participant D as dispatcher

  C->>S: open bi stream + framed envelopes
  S->>S: read_to_end + decode_frames + from_cbor
  alt message_type == command.start
    S->>D: handle_command_start
    D-->>C: command.start response
    D-->>C: command.event*
  else other message
    S->>D: handle_single
    D-->>C: single response
  end
  S-->>C: stream finish
```

## 5. `command.start` 詳細フロー

対象読者: 実装者, 運用者

実装箇所: `crates/imagod/src/protocol_handler.rs` `handle_command_start`

共通処理:

1. `CommandStartRequest` decode + `Validate`
2. envelope `request_id` と payload `request_id` の一致検証（一致しない場合 `E_BAD_REQUEST`）
3. `OperationManager::start`
4. `CommandStartResponse { accepted: true }` 送信
5. `accepted` event 送信
6. `set_state(running, "starting")`
7. `progress(stage="starting")` 送信
8. `mark_spawned_if_not_canceled` で cancel フラグ確認と phase 遷移を原子的に実行

コマンド分岐:

- `deploy` -> `Orchestrator::deploy`
- `run` -> `Orchestrator::run`
- `stop` -> `Orchestrator::stop`

成功時:

- `progress`（詳細 stage）
- `succeeded`
- `finish(succeeded, success_stage)`
- `remove(request_id)`

失敗時:

- `failed(error=StructuredError)`
- `finish(failed, "failed")`
- `remove(request_id)`

spawn 遷移前 cancel 成立時:

- `canceled`
- `finish(canceled, "canceled")`
- `remove(request_id)`
- `mark_spawned_if_not_canceled` の cancel 分岐は terminal state を直接設定せず、イベント送信後に終端化する

## 6. ArtifactStore 詳細

対象読者: 実装者, 運用者

実装箇所: `crates/imagod/src/artifact_store.rs`

### 6.1 データモデル

- `StoreState.sessions: BTreeMap<String, UploadSession>`
- `StoreState.idempotency: BTreeMap<String, String>`
- `UploadSession` 主要項目:
  - `service_name`
  - `idempotency_key`
  - `artifact_digest`, `manifest_digest`, `artifact_size`
  - `upload_token`
  - `received_ranges`
  - `committed`
  - `inflight_writes`
  - `commit_in_progress`
  - `updated_at_epoch_secs`
  - `file_path`

- `ArtifactStore` 追加制約パラメータ:
  - `max_chunk_size`
  - `max_inflight_chunks`
  - `max_artifact_size_bytes`

### 6.2 不変条件

- `prepare`: `artifact_size > 0`
- `prepare`: `artifact_size <= max_artifact_size_bytes`
- `prepare`: idempotency 判定は lock 内で行い、artifact ファイル作成 (`open`/`set_len`/`flush`) は lock 外で行う
- `prepare`: lock 外 I/O 後に lock を再取得して最終挿入し、競合時は作成済みファイルを cleanup plan で削除
- `push`: `upload_token` 一致、range 妥当、`length <= max_chunk_size`、chunk hash 一致
- `push`: decode 前に `chunk_b64` encoded 長を検証し、`header.length` 由来上限を超える入力を拒否
- `push`: `inflight_writes < max_inflight_chunks`（超過時 `E_BUSY`）
- `commit`: metadata 一致、`inflight_writes == 0`、必要 range 完了、digest 一致
- `committed_artifact`: `committed=true` の session のみ返却
- `build_prepare_response`: partial 時の `missing_ranges` は欠損レンジ全件を列挙して返す

### 6.3 GC / 保持方針

- 入口 GC: `prepare` / `push` / `commit` / `committed_artifact`
- TTL 超過の未完了 session を削除（`inflight_writes > 0` / `commit_in_progress` は除外）
- 同一 service の旧コミット artifact/session/idempotency を削除し、最新のみ保持
- lock 内は削除対象の計画生成のみ、実ファイル削除は lock 外で実行
- `prepare` / `push` / `commit` は lock を分割し、ファイル I/O と digest 計算を lock 外で実施

## 7. Orchestrator 詳細

対象読者: 実装者, 運用者

実装箇所: `crates/imagod/src/orchestrator.rs`

主要経路:

- `deploy(payload)`
  - `prepare_release`
  - `supervisor.replace`
  - 成功時 `active_release` 更新
  - 失敗時 `auto_rollback=true` なら rollback 実行
- `run(payload)`
  - `active_release` 読込
  - release の `manifest.json` 再読込
  - `supervisor.start`
- `stop(payload)`
  - `supervisor.stop`

deploy 経路の要点:

- staging 展開
- manifest/hash 検証
- `manifest.name` は `[A-Za-z0-9._-]` のみ許可し、path separator/traversal を拒否
- `manifest.main` は相対パスのみ許可し、絶対/`..`/Windows prefix を拒否
- release ID は `sha256(artifact_digest文字列)` の 64 hex を採用（16桁切り詰めはしない）
- `expected_current_release` は CAS で検証（`any` は比較スキップ、不一致は `E_PRECONDITION_FAILED`）
- `restart_policy` は `never` のみ受理し、他値は `E_BAD_REQUEST`
- `services/<name>/<release_hash>/` 配置
- 旧 release cleanup
- release 配置は `staging -> release` を安全な swap で実施し、失敗時は backup から復元する
- supervisor 起動置換

## 8. ServiceSupervisor 詳細

対象読者: 実装者, 運用者

実装箇所: `crates/imagod/src/service_supervisor.rs`

内部状態:

- `RwLock<BTreeMap<String, RunningService>>`
- `RunningService`:
  - `release_hash`
  - `started_at`
  - `status`
  - `runner_id`
  - `runner_endpoint`
  - `manager_auth_secret`
  - `invocation_secret`
  - `bindings`
  - `child`（`tokio::process::Child`）
  - `stdout/stderr` ring buffer
  - `last_heartbeat_at`
- manager control endpoint:
  - `runtime/ipc/manager-control.sock`
- pending readiness:
  - `pending_ready[runner_id] -> oneshot sender`

主要 API:

- `start`
- `replace`
- `stop(force)`
- `reap_finished`
- `has_live_services`

停止ポリシー:

- `force=false`: `shutdown_runner`（IPC）-> grace timeout 待機 -> 必要なら kill
- `force=true`: 即 kill
- `stop` は待機中に `stopping_count` を加算し、`has_live_services` が false にならないようにする

起動ポリシー:

- `start` は `imagod --runner` を `spawn+exec` し、stdin で `RunnerBootstrap` を渡す
- `start` は spawn 後すぐ成功返却せず、`runner_ready_timeout_secs` 以内の `runner_ready` を待つ
- timeout / ready 前終了時は起動失敗として child を回収し、deploy 側で rollback 経路へ入れる

## 9. Wasmtime 実行詳細

対象読者: 実装者

実装箇所: `crates/imagod/src/runtime_wasmtime.rs`

設定:

- `Config::wasm_component_model(true)`
- `Config::async_support(true)`
- `Config::epoch_interruption(true)`

実行:

- `wasmtime_wasi::p2::add_to_linker_async`
- `wasmtime_wasi::p2::bindings::Command::instantiate_async`
- `call_run(...).await`
- `Store::set_epoch_deadline(1)`
- `Store::epoch_deadline_async_yield_and_update(1)`

停止連携:

- `watch::Receiver<bool>` の shutdown signal と run future を `tokio::select!` で競合実行
- runner 内の epoch tick task が `epoch_tick_interval_ms` 周期で `Engine::increment_epoch()` を呼び、停止時の割り込み余地を維持する

## 10. 状態管理と cancel セマンティクス

対象読者: 実装者, 運用者

実装箇所: `crates/imagod/src/operation_state.rs`

状態モデル:

- `CommandState`: `accepted`, `running`, `succeeded`, `failed`, `canceled`
- `OperationPhase`: `starting` / `spawned`

cancel 境界:

- `starting` かつ `mark_spawned_if_not_canceled` 実行前のみ cancel 可能
- `spawned` 以降は cancel 不可

終端後:

- `protocol_handler` が terminal event 送信後に `remove(request_id)` 実行
- 以後 `state.request` / `command.cancel` は `E_NOT_FOUND`

```mermaid
stateDiagram-v2
  [*] --> accepted
  accepted --> running
  running --> canceled
  running --> succeeded
  running --> failed
  note right of running
    phase: starting -> spawned
    cancel allowed only in starting
  end note
  canceled --> [*]
  succeeded --> [*]
  failed --> [*]
```

## 11. エラーモデル

対象読者: 実装者, 運用者

実装箇所: `crates/imagod/src/error.rs`

`ImagodError` 主要項目:

- `code: ErrorCode`
- `stage: String`
- `message: String`
- `retryable: bool`
- `details`

`to_structured()` で `imago_protocol::StructuredError` へ変換して wire に載せる。

代表 stage:

- `config.load`
- `transport.setup`
- `deploy.prepare`
- `artifact.push`
- `artifact.commit`
- `orchestration`
- `runtime.start`
- `command.start`

## 12. 並行性・メモリ・CPU特性

対象読者: 実装者, 運用者

共有状態:

- `ArtifactStore`: `tokio::Mutex`
- `OperationManager`: `tokio::RwLock`
- `ServiceSupervisor`: `tokio::RwLock`

バックグラウンドタスク:

- session task（session ごとに spawn）
- maintenance loop（単一）
  - `reap_finished_services`
  - live service あり: active interval sleep
  - live service なし: idle 1 秒 sleep
  - shutdown signal を await 境界（reap/has_live/sleep）で優先確認し、停止遅延を抑制
  - shutdown 後は maintenance task の join を待機し、30秒 timeout 超過時は process をエラー終了
- manager control server（単一）
  - `register_runner` / `runner_ready` / `heartbeat` / `resolve_invocation_target`
- runner process 内 task
  - inbound server（`shutdown_runner` / `invoke`）
  - heartbeat sender
  - epoch tick task（`increment_epoch`）

```mermaid
flowchart TD
  A["maintenance loop"] --> B["reap_finished_services"]
  B --> C{"has_live_services"}
  C -->|yes| E["sleep active interval"]
  C -->|no| F["sleep idle 1s"]
  E --> A
  F --> A
```

増加抑制:

- operation: terminal 後に削除
- artifact: TTL GC + 同名旧コミット削除 + orphan idempotency 清掃

## 13. 運用観点

対象読者: 運用者

主要ログ観点:

- 起動: listen addr
- session 異常: stream read/write エラー
- service 異常終了: supervisor の join 結果
- artifact cleanup 異常

典型トラブル起点:

- 接続不可: TLS/mTLS パス不整合
- deploy 失敗: digest/manifest 不一致
- `E_NOT_FOUND`: 終端後照会の可能性
- `E_BUSY`: 同名 service 競合

## 14. 既知の制約・将来拡張

対象読者: 実装者, 運用者

既知制約:

- service 管理は in-memory（再起動で消える）
- upload session index も in-memory（再起動跨ぎ継続なし）
- `state.request` は短命 operation のみ
- event 永続化/再送なし

拡張候補:

- 再起動時 service 自動復元
- restart policy/backoff 追加
- artifact index 永続化
- 長期 service 状態照会 API

## 実装参照インデックス

- 起動/配線: `crates/imagod/src/main.rs`
- 設定: `crates/imagod/src/config.rs`
- transport: `crates/imagod/src/transport.rs`
- protocol handler: `crates/imagod/src/protocol_handler.rs`
- artifact store: `crates/imagod/src/artifact_store.rs`
- orchestrator: `crates/imagod/src/orchestrator.rs`
- service supervisor: `crates/imagod/src/service_supervisor.rs`
- runner process: `crates/imagod/src/runner_process.rs`
- ipc transport: `crates/imagod/src/ipc/*`
- runtime: `crates/imagod/src/runtime_wasmtime.rs`
- operation state: `crates/imagod/src/operation_state.rs`
- error: `crates/imagod/src/error.rs`

## 実装反映ノート（Multi-process Runner / 2026-02-11）

- Wasmtime 実行主体を manager process から runner process へ移行した。
  - `main.rs` は `manager` / `--runner` の2モードで起動する。
  - runner は stdin で受け取る `RunnerBootstrap`（CBOR）を使って初期化する。
- `ServiceSupervisor` は task 監督から child process 監督へ変更した。
  - `tokio::process::Command` で `imagod --runner` を起動する。
  - `start` は `runner_ready_timeout_secs` 内に `runner_ready` を受信するまで待機する。
  - `stop(force=false)` は `shutdown_runner` 要求（IPC）→ grace timeout → kill fallback。
- IPC は `ipc` モジュールに抽象化した。
  - trait: `ControlPlaneTransport`, `InvocationTransport`
  - 実装: `DbusP2pTransport`（UDS 上の frame + CBOR）
  - manager-runner 制御: `register_runner`, `runner_ready`, `shutdown_runner`, `heartbeat`, `resolve_invocation_target`
- runner 間 direct invoke の基盤を追加した（実関数実行は未実装）。
  - `resolve_invocation_target` は `manifest.bindings` を用いた interface 単位 ACL を適用する。
  - manager は target runner 秘密鍵で短命 token を発行し、callee runner が検証する。
- ログ回収を追加した。
  - runner stdout/stderr を pipe で manager が回収する。
  - service ごとに容量上限付き ring buffer（`runner_log_buffer_bytes`）へ保持する。
- epoch 割り込みの駆動点を変更した。
  - 旧: manager の maintenance loop で `increment_epoch`
  - 新: runner 内で `epoch_tick_interval_ms` 周期の tick task が `increment_epoch`
