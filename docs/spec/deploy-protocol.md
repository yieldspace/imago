# Deploy Protocol Specification

## 1. 目的

`imago-cli` と `imagod` の deploy/run/stop 通信契約を固定し、実装差分で wire 互換が壊れないようにする。

関連仕様:

- 設定入力: [`config.md`](./config.md)
- マニフェスト: [`manifest.md`](./manifest.md)
- 観測契約: [`observability.md`](./observability.md)
- 型の正本: [`imago-protocol.md`](./imago-protocol.md)

## 2. トランスポート層

- Transport: QUIC + WebTransport
- Auth: mTLS
- Payload format: CBOR
- Rust 実装: `quinn` + `web-transport-quinn`
- `imago deploy` は接続確立フェーズで証明書認証失敗を `E_UNAUTHORIZED` として報告する（stage: `transport.connect`）。

### 2.1 ストリーム上のフレーミング

現行実装では、stream 内メッセージを次のフレーム形式で送る。

- `4byte big-endian length`
- `CBOR payload`

運用ルール:

- request は **1 stream あたり 1 envelope のみ**許可する。
- `command.start` は同一 stream 上で `command.start response` と `command.event*` を返す（response/event は複数可）。
- request envelope が複数ある stream は `E_BAD_REQUEST` で拒否する。

## 3. 共通封筒（ProtocolEnvelope）

wire 上の共通形:

```json
{
  "type": "command.start",
  "request_id": "550e8400-e29b-41d4-a716-446655440000",
  "correlation_id": "d6f5fbe7-c9c2-4f4b-bc6b-3f17c60f8b9b",
  "payload": {},
  "error": null
}
```

フィールド:

- `type`: `MessageType`（文字列）
- `request_id`: UUID（nil UUID 禁止）
- `correlation_id`: UUID（nil UUID 禁止）
- `payload`: 各メッセージ payload
- `error`: 失敗時のみ `StructuredError`

## 4. プロトコルシーケンス

### 4.1 Deploy（artifact あり）

1. `hello.negotiate`
2. `deploy.prepare`
3. `artifact.push`（必要チャンクのみ）
4. `artifact.commit`
5. `command.start` (`command_type=deploy`)
6. `command.event*`
7. terminal event 受信後に stream close

補足:

- server は request stream の read を 30 秒で timeout 監視し、無期限待機を避ける。
- timeout 時は `E_OPERATION_TIMEOUT` を返し stream を閉じる。

### 4.2 Run / Stop（artifact なし）

1. `hello.negotiate`
2. `command.start` (`command_type=run|stop`)
3. `command.event*`
4. terminal event 受信後に stream close

## 5. メッセージ契約

### 5.1 `hello.negotiate`

request:

- `compatibility_date`（`YYYY-MM-DD`）
- `client_version`
- `required_features`

response:

- `accepted`
- `server_version`
- `features`
- `limits`

`limits` に含まれる主要キー:

- `chunk_size`
- `max_inflight_chunks`
- `max_artifact_size_bytes`
- `upload_session_ttl`

`compatibility_date` は `protocol_draft` に戻さない。
`hello.negotiate` request は unknown field を受理しない（legacy `protocol_draft` を含め拒否）。

### 5.2 `deploy.prepare`

request:

- `name`
- `type`（Rust 型上は `app_type`）
- `target`
- `artifact_digest`
- `artifact_size`
- `manifest_digest`
- `idempotency_key`
- `policy`

response:

- `deploy_id`
- `artifact_status` (`missing` / `partial` / `complete`)
- `missing_ranges`
- `upload_token`
- `session_expires_at`

クライアント挙動:

- `artifact_status=complete`: upload なし
- `artifact_status=missing`: 全体 upload
- `artifact_status=partial`: `missing_ranges` のみ upload（全量再送しない）
- `missing_ranges` は partial 時に「先頭1件」ではなく「全欠損レンジ集合」を返す

### 5.3 `artifact.push`

request payload:

- `deploy_id`
- `offset`
- `length`
- `chunk_sha256`
- `upload_token`
- `chunk_b64`

制約:

- `length <= hello.limits.chunk_size`
- 同一 deploy session の同時 push は `hello.limits.max_inflight_chunks` を上限として `E_BUSY` で制御する。
- `imago-cli` は `hello.limits` の `chunk_size` / `max_inflight_chunks` を実際の upload 送信パラメータに適用する。
- server は decode 前に `chunk_b64` encoded 長を `header.length` 由来の上限で検証し、過大入力を `E_RANGE_INVALID` で拒否する。

response payload (`artifact.push` ack):

- `received_ranges`
- `next_missing_range`
- `accepted_bytes`

### 5.4 `artifact.commit`

request:

- `deploy_id`
- `artifact_digest`
- `artifact_size`
- `manifest_digest`

response:

- `artifact_id`
- `verified`

制約:

- `deploy.prepare.artifact_size <= hello.limits.max_artifact_size_bytes`
- 上限超過時は `E_STORAGE_QUOTA`

### 5.5 `command.start`

request:

- `request_id`（UUID）
- `command_type`（`deploy` / `run` / `stop`）
- `payload`

運用ルール:

- `command.start` は envelope 側 `request_id` と payload 側 `request_id` に同一 UUID を使う。

`payload` は `command_type` と一致必須。

- `deploy`: `deploy_id`, `expected_current_release`, `restart_policy`, `auto_rollback`
- `run`: `name`
- `stop`: `name`, `force`

`deploy` payload の実行条件:

- `expected_current_release = "any"` の場合は比較をスキップする。
- `expected_current_release != "any"` の場合は server 側 `active_release` と完全一致必須。
- 不一致時は `E_PRECONDITION_FAILED` を返す。
- `restart_policy` は現行実装では `never` のみ受理し、それ以外は `E_BAD_REQUEST`。

response:

- `accepted`（bool）

### 5.6 `command.event`

payload:

- `event_type`（`accepted` / `progress` / `succeeded` / `failed` / `canceled`）
- `request_id`
- `command_type`
- `timestamp`
- `stage`（`event_type=progress` で必須）
- `error`（`event_type=failed` で必須）

### 5.7 `state.request` / `state.response`

`state.request` request:

- `request_id`

`state.response` response:

- `request_id`
- `state`
- `stage`
- `updated_at`

制約:

- `state.response.state` は `accepted` / `running` のみ。
- terminal state を返してはならない。
- 対象が非実行中なら `E_NOT_FOUND`。
- `state.request` のエラー応答 envelope `type` も `state.response` を使う。

### 5.8 `command.cancel`

request:

- `request_id`

response:

- `cancellable`
- `final_state`

現行挙動:

- 起動前（spawn 直前の原子的遷移より前）のみ `cancellable=true`。
- 起動後（spawn 後、operation が残っている間）は `cancellable=false`。
- 終端後（operation 削除後）は `E_NOT_FOUND`。

## 6. 状態遷移

`accepted -> running -> succeeded | failed | canceled`

## 7. 構造化エラー

`error` フィールドは `StructuredError` を使う。

- `code`
- `message`
- `retryable`
- `stage`
- `details`（`BTreeMap<String, String>`）

主要コード:

- `E_UNAUTHORIZED`
- `E_BAD_REQUEST`
- `E_BAD_MANIFEST`
- `E_BUSY`
- `E_NOT_FOUND`
- `E_INTERNAL`
- `E_IDEMPOTENCY_CONFLICT`
- `E_RANGE_INVALID`
- `E_CHUNK_HASH_MISMATCH`
- `E_ARTIFACT_INCOMPLETE`
- `E_PRECONDITION_FAILED`
- `E_OPERATION_TIMEOUT`
- `E_ROLLBACK_FAILED`
- `E_STORAGE_QUOTA`

## 8. 既定値

- `auto_rollback = true`
- `chunk_size = 1MiB`
- `max_inflight_chunks = 16`
- `upload_session_ttl = 15m`
- `max_artifact_size_bytes = 64MiB`

## 実装反映ノート（Issue #64 / 2026-02-11）

- `imago-cli` の `deploy` 接続フェーズで、証明書認証失敗（Unknown CA / 不正証明書 / 証明書必須違反など）を `E_UNAUTHORIZED` に正規化する。
- 将来の CONNECT 拒否との整合のため、HTTP status `401` / `403` も `E_UNAUTHORIZED` として扱う。
- 対象は CLI のエラー正規化のみで、mTLS 検証位置（TLS handshake）および protocol wire 契約は変更しない。
