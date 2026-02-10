# Deploy Protocol Specification

## 目的

CLI と daemon 間の deploy 契約を単一仕様に固定し、再送・再接続・失敗復旧の判断を実装者に残さない。

関連仕様:

- 設定入力: [`config.md`](./config.md)
- manifest 契約: [`manifest.md`](./manifest.md)
- operation イベント契約: [`observability.md`](./observability.md)

## トランスポートと認証

- Transport: QUIC + WebTransport
- Data format: CBOR
- AuthN/AuthZ: mTLS

## 共通メッセージ封筒

```json
{
  "type": "deploy.prepare",
  "request_id": "uuid",
  "correlation_id": "uuid",
  "payload": {},
  "error": null
}
```

### 共通フィールド

- `type`: メッセージ種別
- `request_id`: 要求単位の識別子
- `correlation_id`: deploy / operation 追跡識別子
- `payload`: 本文
- `error`: 失敗時のみ。形式は「構造化エラー契約」を参照

## プロトコルステップ

1. `hello.negotiate`
2. `deploy.prepare`
3. `artifact.push`（必要チャンクのみ）
4. `artifact.commit`
5. `deploy.execute`
6. `operation.watch` または `operation.get`

<a id="message-contracts"></a>
## メッセージ契約

### `hello.negotiate`

request:

- `protocol_draft`
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

### `deploy.execute`

request:

- `deploy_id`
- `expected_current_release`
- `restart_policy`
- `auto_rollback`

response:

- `operation_id`
- `state` (`accepted`)

### `operation.get`

request:

- `operation_id`

response:

- 最新スナップショット（詳細は [`observability.md`](./observability.md)）

### `operation.watch`

request:

- `operation_id`
- `cursor` (任意)
- `limit` (任意)

response:

- イベント列（詳細は [`observability.md`](./observability.md)）

### `operation.cancel`

request:

- `operation_id`

response:

- `cancellable`
- `final_state`

<a id="state-machine"></a>
## 状態遷移

`connected -> negotiated -> prepared -> uploading -> committed -> executing -> succeeded | failed | rolled_back`

<a id="error-contract"></a>
## 構造化エラー契約

```json
{
  "code": "E_CHUNK_HASH_MISMATCH",
  "message": "chunk digest mismatch",
  "retryable": true,
  "stage": "artifact.push",
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

operation の保持・再取得契約は [`observability.md`](./observability.md) を参照。

## 非対象

- blue-green デプロイ
- 差分配信
