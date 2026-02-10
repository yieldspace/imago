# Deploy Protocol Specification

## 目的

CLI と daemon 間の deploy 仕様を単一仕様に固定し、micro linux 環境でも低コストで運用できるようにする。

関連仕様:

- 設定入力: [`config.md`](./config.md)
- manifest 仕様: [`manifest.md`](./manifest.md)
- 観測性仕様: [`observability.md`](./observability.md)

## トランスポートと認証

- Transport: QUIC + WebTransport
- Data format: CBOR
- AuthN/AuthZ: mTLS
- Rust実装: `quinn` + `web-transport-quinn`

## フレーミング

- 1 stream で複数メッセージを送るため、`4byte BE length + CBOR payload` のフレーム形式を使う。
- `command.start` は同一 stream で `command.start response` の後に `command.event*` を送る。

## 共通メッセージ封筒

```json
{
  "type": "command.start",
  "request_id": "uuid",
  "correlation_id": "uuid",
  "payload": {},
  "error": null
}
```

### 共通フィールド

- `type`: メッセージ種別
- `request_id`: コマンド実行単位の識別子
- `correlation_id`: ログ相関用識別子
- `payload`: 本文
- `error`: 失敗時のみ。形式は「構造化エラー仕様」を参照

## プロトコルステップ

### Deploy（artifact あり）

1. `hello.negotiate`
2. `deploy.prepare`
3. `artifact.push`（必要チャンクのみ）
4. `artifact.commit`
5. `command.start` (`command_type=deploy`)
6. `command.event*`（同一 stream で push、短命）
7. spawn 成功時点で terminal event（`succeeded`）を返し、クライアントが stream close
8. 必要時のみ `state.request`（現在状態の一点照会）

### Run / Stop（artifact なし）

1. `hello.negotiate`
2. `command.start` (`command_type=run|stop`)
3. `command.event*`（同一 stream で push、短命）
4. terminal event 受信後にクライアントが stream close
5. 必要時のみ `state.request`

<a id="message-contracts"></a>
## メッセージ仕様

### `hello.negotiate`

request:

- `compatibility_date`（`YYYY-MM-DD`）
- `client_version`
- `required_features`

response:

- `accepted`
- `server_version`
- `features`
- `limits`

### `deploy.prepare`

request:

- `name`
- `type`
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

### `artifact.push`

chunk header:

- `deploy_id`
- `offset`
- `length`
- `chunk_sha256`
- `upload_token`
- `chunk_b64`

ack:

- `received_ranges`
- `next_missing_range`
- `accepted_bytes`

### `artifact.commit`

request:

- `deploy_id`
- `artifact_digest`
- `artifact_size`
- `manifest_digest`

response:

- `artifact_id`
- `verified`

### `command.start`

request:

- `request_id`
- `command_type` (`deploy` / `run` / `stop`)
- `payload`

`payload` の必須キー:

- `deploy`: `deploy_id`, `expected_current_release`, `restart_policy`, `auto_rollback`
- `run`: `name`
- `stop`: `name`, `force`

response:

- `accepted`（bool）

### `command.event`

push event payload:

- `event_type` (`accepted` / `progress` / `succeeded` / `failed` / `canceled`)
- `request_id`
- `command_type`
- `timestamp`
- `stage`（`event_type=progress` のとき必須）
- `error`（`event_type=failed` のとき必須）

順序保証は同一 stream の受信順のみ。
`deploy` / `run` の `succeeded` は Wasm プロセス終了ではなく spawn 成功を意味する。

### `state.request`

request:

- `request_id`

response:

- `request_id`
- `state`
- `stage`
- `updated_at`

対象が実行中でない場合は `E_NOT_FOUND`。

### `command.cancel`

request:

- `request_id`

response:

- `cancellable`
- `final_state`

起動前（spawn 前）のみ `cancellable=true`。spawn 後は `cancellable=false` を返す。

<a id="state-machine"></a>
## 状態遷移

`accepted -> running -> succeeded | failed | canceled`

<a id="error-contract"></a>
## 構造化エラー仕様

```json
{
  "code": "E_BAD_REQUEST",
  "message": "invalid command payload",
  "retryable": false,
  "stage": "command.start",
  "details": {}
}
```

### 必須フィールド

- `code`
- `message`
- `retryable`
- `stage`
- `details`

### エラーコード

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

`E_NOT_FOUND` は `state.request` / `command.cancel` で対象 `request_id` が実行中でない場合にも返す。

<a id="idempotency-and-cas"></a>
## 冪等性と前提条件

- `idempotency_key` は `deploy.prepare` で必須。
- 同一 key + 同一入力は同一 `deploy_id` を返す。
- 同一 key + 異なる入力は `E_IDEMPOTENCY_CONFLICT`。
- `expected_current_release` 不一致は `E_PRECONDITION_FAILED`。

<a id="protocol-defaults"></a>
## 既定値

- `auto_rollback = true`
- `chunk_size = 1MiB`
- `max_inflight_chunks = 16`
- `upload_session_ttl = 15m`

## セッション運用方針

- 現行 CLI では 1 実行ごとに 1 WebTransport セッションを作成し、完了後に閉じる。
- 将来は 1 セッション内で複数 stream を開き並列実行してよい。

## 非対象

- blue-green デプロイ
- 差分配信
- イベント履歴の永続保存と再送

## 実装反映ノート（Milestone Phase 1 / 2026-02-10）

- `imago-protocol` の共通封筒実装では `request_id` / `correlation_id` を UUID として扱い、nil UUID を検証で拒否する。
- `deploy.prepare.idempotency_key` は必須かつ空文字を拒否する。
- `artifact.push.length` と `artifact.commit.artifact_size` は 0 を拒否する。
- `command.start` は `command_type` と payload 形状の不一致を拒否する。
- `deploy` payload の `auto_rollback` は未指定時に `true` を既定値として適用する。
- 構造化エラーの `ErrorCode` は列挙値以外をデコード時に拒否する。
