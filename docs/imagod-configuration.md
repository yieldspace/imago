# `imagod.toml` Configuration Reference

## 目的と適用範囲

このドキュメントは、`imagod.toml` の各キーを実運用向けに参照するためのリファレンスです。  
実装契約の正本は [`docs/spec/config.md`](./spec/config.md) で、本ページは「読みやすさ重視」の補助資料です。

## 最小構成サンプル

```toml
[tls]
server_key = "/absolute/path/to/server.key"
client_public_keys = []
```

- `tls.server_key` と `tls.client_public_keys` は必須です。
- `imagod.toml` 未存在で起動した場合、実装は最小有効構成を自動生成できます（詳細は仕様正本を参照）。

## セクション一覧

- トップレベルキー (`listen_addr`, `storage_root`, `server_version`, `compatibility_date`)
- `[tls]`
- `[runtime]`
- 廃止/非対応キー

## キーごとのリファレンス

### トップレベルキー

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `listen_addr` | 任意 | string | server bootstrap 時に `SocketAddr` として parse 可能であること | `"[::]:4443"` | `listen_addr = "[::]:4443"` |
| `storage_root` | 任意 | string(path) | load-time では追加制約なし | 優先順: `imagod.toml` 明示値 > `IMAGOD_STORAGE_ROOT_DEFAULT` > OS既定値 | `storage_root = "/var/lib/imago"` |
| `server_version` | 任意 | string | load-time では追加制約なし | `"imagod/0.1.0"` | `server_version = "imagod/0.1.0"` |
| `compatibility_date` | 任意 | string | `YYYY-MM-DD`（month 1..12, day 1..31） | `"2026-02-10"` | `compatibility_date = "2026-02-10"` |

`storage_root` の OS 既定値:

- Linux: `/var/lib/imago`
- macOS: `/usr/local/var/imago`
- Windows: `C:\ProgramData\imago`
- その他: `/var/lib/imago`

### `[tls]`

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `tls.server_key` | 必須 | string(path) | load-time では追加制約なし（runtime bootstrap で鍵材読み込み） | なし | `server_key = "/absolute/path/to/server.key"` |
| `tls.admin_public_keys` | 任意 | array(string) | 各要素は ed25519 raw public key hex (64 hex chars)、重複不可、`tls.client_public_keys` との重複不可 | `[]` | `admin_public_keys = ["2222...2222"]` |
| `tls.client_public_keys` | 必須 | array(string) | 各要素は ed25519 raw public key hex (64 hex chars)、重複不可、空配列は許容 | なし | `client_public_keys = []` |
| `tls.known_public_keys` | 任意 | table<string,string> | key(authority) は trim 後非空、value は ed25519 raw public key hex (64 hex chars) | `{}` | `known_public_keys = { "rpc://node-a:4443" = "aaaa...aaaa" }` |

`tls.server_key` の自動生成挙動:

- `imagod.toml` 自動生成時は `<config_dir>/server.key` の絶対パスが入ります。
- 該当ファイルが無ければ `server.key` 実体も同時生成されます。

### `[runtime]`

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `runtime.chunk_size` | 任意 | integer (`usize`) | `1..=8388608` | `1048576` | `chunk_size = 1048576` |
| `runtime.max_inflight_chunks` | 任意 | integer (`usize`) | `>=1` | `16` | `max_inflight_chunks = 16` |
| `runtime.max_artifact_size_bytes` | 任意 | integer (`u64`) | `>=1` | `67108864` | `max_artifact_size_bytes = 67108864` |
| `runtime.upload_session_ttl_secs` | 任意 | integer (`u64`) | load-time 検証なし | `900` | `upload_session_ttl_secs = 900` |
| `runtime.stop_grace_timeout_secs` | 任意 | integer (`u64`) | `>=1` | `30` | `stop_grace_timeout_secs = 30` |
| `runtime.runner_ready_timeout_secs` | 任意 | integer (`u64`) | `>=1` | `3` | `runner_ready_timeout_secs = 3` |
| `runtime.runner_log_buffer_bytes` | 任意 | integer (`usize`) | `>=1` | `262144` | `runner_log_buffer_bytes = 262144` |
| `runtime.epoch_tick_interval_ms` | 任意 | integer (`u64`) | `>=1` | `50` | `epoch_tick_interval_ms = 50` |
| `runtime.http_worker_count` | 任意 | integer (`u32`) | `1..=4` | `2` | `http_worker_count = 2` |
| `runtime.http_worker_queue_capacity` | 任意 | integer (`u32`) | `1..=16` | `4` | `http_worker_queue_capacity = 4` |
| `runtime.manager_control_read_timeout_ms` | 任意 | integer (`u64`) | `>=1` | `500` | `manager_control_read_timeout_ms = 500` |
| `runtime.max_concurrent_sessions` | 任意 | integer (`u32`) | `>=1` | `256` | `max_concurrent_sessions = 256` |
| `runtime.deploy_stream_timeout_secs` | 任意 | integer (`u64`) | `>=1` | `15` | `deploy_stream_timeout_secs = 15` |

## 型別・用途別の注意点

### TLS allowlist 運用

- `client_public_keys` は client 証明に使う allowlist です。
- `admin_public_keys` は管理用途キーの allowlist で、`client_public_keys` と混在させません。
- `known_public_keys` は authority ごとの pin 情報です。

### timeout / buffer 調整

- `runtime.runner_ready_timeout_secs` は起動遅延切り分けで最初に調整する値です。
- `runtime.runner_log_buffer_bytes` はメモリ使用量に直結するため、埋め込み環境では慎重に増やします。
- `runtime.max_artifact_size_bytes` は転送失敗時の上限判定に影響します。

### worker 関連

- `runtime.http_worker_count` と `runtime.http_worker_queue_capacity` は同時に調整します。
- 上限を超える値は load-time でエラーになります。

## エラーになりやすい設定例

### 1. 必須 TLS キー欠落

```toml
[tls]
client_public_keys = []
```

`tls.server_key` が必須のためエラーになります。

### 2. 公開鍵フォーマット不正

```toml
[tls]
server_key = "/tmp/server.key"
client_public_keys = ["abcd"]
```

公開鍵は 64 hex chars の ed25519 raw key が必要です。

### 3. runtime の範囲外値

```toml
[runtime]
http_worker_count = 8
```

`runtime.http_worker_count` は `1..=4` のみ受理されます。

### 4. 廃止キーの使用

```toml
protocol_draft = "imago-mvp-v1"

[tls]
server_cert = "server.crt"
client_ca_cert = "ca.crt"
```

これらは非対応/廃止キーのため load エラーになります。

## 廃止/非対応キー

以下は受理されません（load エラー）。

- `protocol_draft`
- `tls.server_cert`
- `tls.client_ca_cert`

## 関連仕様リンク

- 正本: [`docs/spec/config.md`](./spec/config.md)
- deploy/protocol 観点: [`docs/spec/deploy-protocol.md`](./spec/deploy-protocol.md)
- `imagod` 概要: [`docs/spec/imagod.md`](./spec/imagod.md)
