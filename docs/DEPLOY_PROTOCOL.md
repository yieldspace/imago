# Core ↔ Deploy Protocol（刷新ドラフト）

このドキュメントは、imago CLI（デプロイ基盤）と imagod（コア）をつなぐ deploy プロトコルの最新版ドラフト。
旧案（`deploy.begin -> upload -> apply -> restart`）を置換し、再開可能アップロードと非同期実行を前提にする。

## 目的
- 不安定回線でも復旧可能なデプロイを提供する
- 冪等性と前提条件チェックを明示し、重複実行や競合更新を防ぐ
- 長時間処理を非同期 Operation で追跡可能にする
- 失敗時の自動ロールバックを契約化し、運用復旧を簡素化する

## 現行案の問題点
1. `deploy.begin` の再送時に冪等性が弱く、重複処理が起きうる
2. upload が単発ストリーム前提で、中断時に全量再送が必要
3. `apply/restart` が同期前提で、再接続後に追跡しにくい
4. manifest が begin payload と tar 内で二重管理され不整合余地がある
5. 失敗時ロールバックのデフォルト挙動が契約化されていない
6. エラーが粗く、再試行可否や失敗段階の判断が難しい

## 設計原則
1. 互換維持より実装可能性を優先し、破壊的変更を許容する
2. `idempotency_key` で冪等性を担保する
3. artifact 転送はチャンク再開型にする
4. 実行は非同期 Operation 型とする
5. `auto_rollback = true` を既定値にする

## 役割
- **client**: `imago` CLI（build/deploy の実行主体）
- **server**: `imagod`（受信・検証・配置・起動・ロールバック）

## トランスポート
- QUIC + WebTransport + CBOR
- mTLS（接続時の相互認証）

### チャンネル設計
- **Control stream**: request/response/events
- **Data stream**: artifact chunk 転送

## 共通メッセージ形式

```json
{
  "type": "deploy.prepare",
  "request_id": "req-uuid",
  "correlation_id": "deploy-or-op-id",
  "payload": {},
  "error": null
}
```

### 共通フィールド
- `type`: メッセージ種別
- `request_id`: リクエスト識別子（UUID推奨）
- `correlation_id`: deploy/operation の追跡ID
- `payload`: メッセージ本文
- `error`: 失敗時のみ。`code/message/retryable/stage/details`

## メッセージ定義

### 1) `hello.negotiate`
**目的**: 接続初期化と制約確定
- request payload:
  - `protocol_draft`
  - `client_version`
  - `required_features`
- response payload:
  - `accepted`（bool）
  - `server_version`
  - `features`
  - `limits`（chunk上限、inflight上限など）

### 2) `deploy.prepare`
**目的**: デプロイセッション作成と欠損レンジ決定
- request payload:
  - `name`
  - `type`（cli/http/socket）
  - `target`（host/group）
  - `artifact_digest`（sha256）
  - `artifact_size`
  - `manifest_digest`（sha256）
  - `idempotency_key`
  - `policy`（restart_policy / auto_rollback 等）
- response payload:
  - `deploy_id`
  - `artifact_status`（`missing`/`partial`/`complete`）
  - `missing_ranges`
  - `upload_token`
  - `session_expires_at`

### 3) `artifact.push`
**目的**: チャンク転送（再開対応）
- chunk header:
  - `deploy_id`
  - `offset`
  - `length`
  - `chunk_sha256`
  - `upload_token`
- ack event payload:
  - `received_ranges`
  - `next_missing_range`
  - `accepted_bytes`

### 4) `artifact.commit`
**目的**: artifact の最終検証
- request payload:
  - `deploy_id`
  - `artifact_digest`
  - `artifact_size`
  - `manifest_digest`
- response payload:
  - `artifact_id`
  - `verified`（bool）

### 5) `deploy.execute`
**目的**: 配置 + 起動を非同期 Operation として開始
- request payload:
  - `deploy_id`
  - `expected_current_release`（CAS）
  - `restart_policy`
  - `auto_rollback`
- response payload:
  - `operation_id`
  - `state`（`accepted`）

### 6) `operation.get` / `operation.watch`
**目的**: 進捗取得と追跡
- response payload:
  - `operation_id`
  - `stage`（`validate`/`expand`/`cleanup`/`start`/`rollback` 等）
  - `progress`（0-100）
  - `release_id`
  - `process_id`
  - `rollback_status`
  - `error`

### 7) `operation.cancel`
**目的**: 実行中断要求
- request payload:
  - `operation_id`
- response payload:
  - `cancellable`
  - `final_state`

## 状態遷移
1. `connected`
2. `negotiated`
3. `prepared`
4. `uploading`（resumable）
5. `committed`
6. `executing`（async）
7. `succeeded` / `failed` / `rolled_back`

## エラー契約
全エラーは以下形式で返す。

```json
{
  "code": "E_CHUNK_HASH_MISMATCH",
  "message": "chunk digest mismatch",
  "retryable": true,
  "stage": "artifact.push",
  "details": {}
}
```

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

## 既定値
- `auto_rollback = true`
- `chunk_size = 1MiB`
- `max_inflight_chunks = 16`
- `upload_session_ttl = 15m`
- `operation_retention = 24h`

## シーケンス例
1. `hello.negotiate`
2. `deploy.prepare`
3. `artifact.push`（必要レンジのみ）
4. `artifact.commit`
5. `deploy.execute`
6. `operation.watch`（完了まで）

## 検証ルール
- tar.gz 内の `manifest.json` は必須
- manifest の正本は tar.gz 内とし、`manifest_digest` で照合
- `artifact_digest` と `artifact_size` は必須
- `idempotency_key` は必須
- `expected_current_release` が不一致なら `E_PRECONDITION_FAILED`

## 参考: build/ の最小構成
- `manifest.json`
- `app.wasm`
- `imago.lock`（任意）
