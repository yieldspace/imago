# imagod Server Specification (Overview)

## 1. 目的

`imagod` は deploy protocol のサーバ実装であり、`imago-cli` からの要求を受けて artifact 受領・配置・Wasm 実行管理を行う。

このページは概要層のみを扱う。内部構造の正本は [`imagod-internals.md`](./imagod-internals.md)。

## 2. 責務境界

`imagod` の責務:

- QUIC + WebTransport セッション受理
- `ProtocolEnvelope` (`MessageType`) の decode/dispatch
- RPK 認証（クライアント公開鍵 allowlist で認可）
- `deploy.prepare` / `artifact.push` / `artifact.commit`
- `command.start` (`deploy` / `run` / `stop`) と `command.event` 配信
- `type=rpc` runner の常駐管理（起動時は `main` 非実行、`rpc.invoke` で関数実行）
- `state.request -> state.response` の実行中状態照会
- `command.cancel` の起動前 cancel 判定
- manager/runner のマルチプロセス実行制御（1 service = 1 runner process）
- manager-runner / runner-runner の IPC（DBus over UDS, trait 抽象）
- runner stdout/stderr のパイプ回収とメモリ上限付きバッファ保持
- manager 起動時の `restart_policy=always` service 自動復元（best-effort）
- plugin component の SHA-256 検証、cache 再利用、起動時 GC
- app/plugin capability ルールに基づく plugin import 認可（default deny）

`imagod` の非責務（または未実装）:

- イベント永続化・再送
- 高度な restart policy
- blue-green デプロイ
- RPC 呼び出し結果の永続化・再送

## 3. 外部仕様との対応

- 通信仕様: [`deploy-protocol.md`](./deploy-protocol.md)
- 観測仕様: [`observability.md`](./observability.md)
- 設定仕様: [`config.md`](./config.md)
- protocol 型仕様: [`imago-protocol.md`](./imago-protocol.md)

## 4. 互換キー方針

`hello.negotiate` では `compatibility_date` を使う。

- 既定値: `2026-02-10`
- 判定: 現行は文字列一致
- `protocol_draft` は受理しない

## 5. 設定サマリー（`imagod.toml`）

```toml
listen_addr = "[::]:4443"
storage_root = "/var/lib/imago"
server_version = "imagod/0.1.0"
compatibility_date = "2026-02-10"

[tls]
server_key = "/etc/imago/server.key"
admin_public_keys = ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
client_public_keys = []
known_public_keys = { "rpc://node-a:4443" = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }

[runtime]
chunk_size = 1048576
max_inflight_chunks = 16
upload_session_ttl_secs = 900
stop_grace_timeout_secs = 30
runner_ready_timeout_secs = 3
runner_log_buffer_bytes = 262144
epoch_tick_interval_ms = 50
```

`storage_root` の未指定時既定値は OS とビルド時設定で変わる。優先順位と OS 別値は [`config.md`](./config.md) を参照。

`runtime.runner_log_buffer_bytes` は runner stdout/stderr の保持に加え、停止済みサービスの retained logs を保持する global ring の総量上限としても使う。

起動時に解決された設定パス（既定は `/etc/imago/imagod.toml`、`--config` / `IMAGOD_CONFIG` 指定時はそのパス）の `imagod.toml` が存在しない場合、`imagod` は最小有効構成を自動生成して起動を継続する。

- 既存ファイルがある場合は上書きしない。
- 自動生成時は設定ファイルを生成した旨を起動ログで通知する。
- 自動生成される `imagod.toml` には英語コメントを含める。
- 自動生成対象は `listen_addr` / `server_version` / `compatibility_date` / `tls.server_key` / `tls.client_public_keys`。
- `tls.server_key` は自動生成した `imagod.toml` と同じディレクトリの `server.key`（絶対パス）を指す。該当ファイルが未存在なら、`imagod.toml` 自動生成と同時に `server.key` 実体も生成する。
- `tls.client_public_keys` は空配列 `[]` を許容し、運用者が必要な公開鍵を追記する。

詳細は [`config.md`](./config.md) を参照。

## 6. 実装追従方針

- 概要ページは責務境界と外部契約の橋渡しに限定する。
- 内部挙動は `crates/imagod/src/lib.rs`（dispatch API）/ `crates/imagod/src/main.rs`（thin entrypoint）と `crates/imagod-*/src/*` の関数/型名で追跡し、[`imagod-internals.md`](./imagod-internals.md) を更新する。
- `imago-protocol` 側の型・検証契約を変更した場合、`imagod` 側ドキュメントを同時に更新する。

## 実装反映ノート（Crate Split 6+1 / 2026-02-11）

- `imagod` の内部構成を単一 crate から 6+1 構成（`imagod` + `imagod-*`）へ分割した。
- 外部公開の実行形式は維持し、`imagod` バイナリ名と `imagod --runner` は不変。
- deploy protocol / manifest の外部 wire 契約は変更せず、内部責務分離のみ実施した。

## 実装反映ノート（Boot Restore / 2026-02-14）

- manager 起動時に `storage_root/services/<service>/active_release` を走査し、service 名昇順で自動起動する。
- 復元対象は `restart_policy` が `always` で、かつ `active_release` が存在する service のみ。
- `restart_policy` ファイルが欠落している service は `never` として扱い、起動しない。
- 一部 service の復元失敗はログへ記録して起動を継続する（best-effort）。

## 実装反映ノート（Plugin Runtime MVP-1 / 2026-02-17）

- deploy 時に `manifest.dependencies(kind=wasm)` の `component.sha256` を検証し、`storage_root/plugins/components/<sha256>.wasm` へキャッシュ配置する。
- 同一 hash の plugin component は再配置せず再利用する。
- manager 起動時に active release の manifest を走査し、未参照 plugin component を GC する。
- `RunnerBootstrap` に plugin 依存定義と capability ルールを含め、runner runtime へ伝播する。
- Wasmtime runtime は dependency import に対して `func_new_async` bridge を構成し、解決順は `self(component export)` -> `明示 dependency(package名一致)` -> `error`。
- capability は明示 dependency への中継時のみ `deps` で評価し、self 解決は認可不要とする。
- transitive import 解決では `requires` の記述を必須条件にしない。

## 実装反映ノート（Native Plugin imago:admin / 2026-02-17）

- `imagod` runner は trait/dyn ベースの native plugin registry を持ち、起動時に明示登録された plugin を利用する。
- native plugin descriptor（package/import/symbol/add_to_linker）は WIT から macro で生成する。
- `imago:admin` 実装は workspace 直下 `plugins/imago-admin` crate に分離した。
- `imago:admin/runtime@0.1.0` import は Wasmtime `component::bindgen!` 生成の `add_to_linker` で解決する。
- native plugin API は read-only 4 関数のみ提供する。
  - `service-name() -> string`
  - `release-hash() -> string`
  - `runner-id() -> string`
  - `app-type() -> string`（`cli` / `rpc` / `http` / `socket`）
- `manifest.dependencies(kind=native)` で `name="imago:admin"` を宣言した場合、既存 capability ルール（`capabilities.deps`）をそのまま適用する。
- `kind=native` dependency が registry 未登録の場合は、component import 解決前に起動時エラーで停止する。
## 実装反映ノート（Storage Root Default Matrix / 2026-02-14）

- `imagod.toml` の `storage_root` 未指定時既定値を固定 `/etc/imago` から OS 別既定値へ変更した（Linux=`/var/lib/imago`, macOS=`/usr/local/var/imago`, Windows=`C:\ProgramData\imago`, その他=`/var/lib/imago`）。
- ビルド時環境変数 `IMAGOD_STORAGE_ROOT_DEFAULT` を指定した場合は、その値を `storage_root` 既定値として採用する。
- `imagod.toml` に `storage_root` を明示した場合は、従来どおり明示値を最優先する。

## 実装反映ノート（RPK + TOFU / 2026-02-18）

- [BREAKING] `imagod.toml` の `tls.server_cert` / `tls.client_ca_cert` を廃止し、`tls.server_key` / `tls.client_public_keys` へ移行した。
- `imagod` はサーバ証明書チェーンを持たず、`server_key` から提示する RPK で識別される。
- クライアント側は `known_hosts` でサーバ鍵 pin を行い、初回接続時のみ TOFU 登録する。

## 実装反映ノート（Network RPC / 2026-02-18）

- `tls.admin_public_keys` を追加し、管理操作（deploy/run/stop/logs/state/cert）を許可する鍵を分離した。
- `tls.client_public_keys` は RPC クライアント鍵として扱い、`hello.negotiate` と `rpc.invoke` のみ許可する。
- `tls.known_public_keys` は `authority -> public key` table へ変更した。

## 実装反映ノート（RPC resident runner startup / 2026-02-19）

- `type=rpc` は runner 起動時に `main` を自動実行せず、shutdown まで常駐待機する。
- `rpc.invoke` が到着したタイミングでのみ、`manifest.main` が指す component の関数を実行する。

## 実装反映ノート（Retained logs 契約 / 2026-02-20）

- `logs.request.name=None` は running サービスに加え、retained logs が残る停止済みサービスも対象に含める。
- retained logs は imagod プロセス寿命内メモリの global ring にのみ保持し、eviction またはプロセス再起動後は参照できない。
- 停止済みサービスへ `follow=true` を指定した場合は snapshot 後に `logs.end` で即終了する。
- `runtime.runner_log_buffer_bytes` は retained global ring の総量上限にも適用される。

## 実装反映ノート（imagod.toml 自動生成 / 2026-02-21）

- manager 起動時に `imagod.toml` が未存在なら、英語コメント付きの最小有効構成（`listen_addr` / `server_version` / `compatibility_date` / `tls.server_key` / `tls.client_public_keys`）を自動生成して起動を継続する。
- 既存の `imagod.toml` は上書きしない。
- 自動生成時は起動ログで通知する。
- `tls.server_key` は自動生成した `imagod.toml` と同じディレクトリの `server.key`（絶対パス）を指し、該当ファイルが未存在なら `imagod.toml` 自動生成と同時に `server.key` 実体も生成する。
- `tls.client_public_keys` は空配列 `[]` を許容する。

## 実装反映ノート（Library Dispatch + Native Plugin Registry API / 2026-02-21）

- `crates/imagod` は `lib + bin` 構成になり、`imagod::dispatch_from_env()` / `imagod::dispatch_from_env_with_registry(...)` を公開した。
- `crates/imagod/src/main.rs` は `imagod::dispatch_from_env()` へ委譲する thin entrypoint に変更した。
- `imagod` library は `register_builtin_native_plugins(...)` / `builtin_native_plugin_registry()` を公開し、built-in（`imago:admin`, `imago:node`）へ追加 native plugin を静的登録で上乗せできる。
- manager 側の process model（`current_exe --runner`）は維持し、未登録 `kind=native` dependency の起動時エラー契約は変更しない。
