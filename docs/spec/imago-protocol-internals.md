# imago-protocol Internal Reference

この文書は `imago-protocol` crate の内部構造を、公開契約と実装詳細の対応が追える粒度で記述する。

- 対象コード: `crates/imago-protocol/src/*.rs`
- 概要仕様: [`imago-protocol.md`](./imago-protocol.md)
- 外部契約: [`deploy-protocol.md`](./deploy-protocol.md), [`observability.md`](./observability.md)

## 1. Scope / 読み方

- 本文書は「型定義」と「`Validate` 契約」の正確な把握を目的とする。
- transport 実装や daemon 側状態遷移は対象外。
- 実装参照はコード断片ではなく `ファイル + 型/関数名` を主とする。

## 2. モジュール構成

| モジュール | 主責務 | 主な公開要素 |
|---|---|---|
| `lib.rs` | 公開 API の再 export | `ProtocolEnvelope`, `MessageType`, `Validate` など |
| `cbor.rs` | CBOR codec | `to_cbor`, `from_cbor`, `CborError` |
| `envelope.rs` | 共通封筒型 | `ProtocolEnvelope<T>` |
| `error.rs` | 構造化エラー契約 | `ErrorCode`, `StructuredError` |
| `messages.rs` | メッセージ本体型 | `HelloNegotiateRequest` ほか全 request/response |
| `validate.rs` | 汎用バリデーション基盤 | `Validate`, `ValidationError` |

## 3. CBOR レイヤー（`cbor.rs`）

- `to_cbor<T: Serialize>` は `Vec<u8>` を返す。
- `from_cbor<T: DeserializeOwned>` は `T` を返す。
- エラーは `CborError::Encode` / `CborError::Decode` に統一する。

設計意図:

- 送受信層は `imago-protocol` の CBOR API のみを利用し、codec 実装差を閉じ込める。

## 4. 共通封筒（`envelope.rs`）

`ProtocolEnvelope<TPayload>` のフィールド:

- `message_type`（serde rename で wire key `"type"`）
- `request_id: Uuid`
- `correlation_id: Uuid`
- `payload: TPayload`
- `error: Option<StructuredError>`

`Validate` 契約:

- `request_id` が nil UUID なら失敗。
- `correlation_id` が nil UUID なら失敗。
- `payload.validate()` を必ず実行。
- `error` が存在する場合は `error.validate()` を実行。

## 5. 構造化エラー（`error.rs`）

### 5.1 `ErrorCode`

列挙値（wire rename）:

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

### 5.2 `StructuredError`

フィールド:

- `code: ErrorCode`
- `message: String`
- `retryable: bool`
- `stage: String`
- `details: BTreeMap<String, String>`

`Validate` 契約:

- `message` 非空必須
- `stage` 非空必須

## 6. メッセージ契約（`messages.rs`）

### 6.1 共通 enum

- `MessageType`
  - `hello.negotiate`
  - `deploy.prepare`
  - `artifact.push`
  - `artifact.commit`
  - `command.start`
  - `command.event`
  - `state.request`
  - `state.response`
  - `command.cancel`
  - `logs.request`
  - `logs.chunk`
  - `logs.end`
- `ArtifactStatus`: `missing` / `partial` / `complete`
- `CommandType`: `deploy` / `run` / `stop`
- `CommandEventType`: `accepted` / `progress` / `succeeded` / `failed` / `canceled`
- `CommandState`: `accepted` / `running` / `succeeded` / `failed` / `canceled`

### 6.2 hello

`HelloNegotiateRequest`:

- `compatibility_date: String`
- `client_version: String`
- `required_features: Vec<String>`
- `#[serde(deny_unknown_fields)]` を付与し、unknown field（例: `protocol_draft`）を decode 拒否する

`Validate`:

- `compatibility_date` 非空
- `client_version` 非空

`HelloNegotiateResponse`:

- `accepted: bool`
- `server_version: String`
- `features: Vec<String>`
- `limits: BTreeMap<String, String>`

`Validate`:

- `server_version` 非空

### 6.3 deploy prepare / artifact

`DeployPrepareRequest`:

- `name`
- `app_type`（wire key `"type"`）
- `target: BTreeMap<String, String>`
- `artifact_digest`
- `artifact_size`
- `manifest_digest`
- `idempotency_key`
- `policy: BTreeMap<String, String>`

`Validate`:

- `name`, `app_type`, `artifact_digest`, `manifest_digest`, `idempotency_key` 非空
- `artifact_size > 0`

`DeployPrepareResponse`:

- `deploy_id`
- `artifact_status`
- `missing_ranges`
- `upload_token`
- `session_expires_at`

`Validate`:

- `deploy_id`, `upload_token`, `session_expires_at` 非空
- `artifact_status=partial` のとき `missing_ranges` 非空
- `missing_ranges` の各要素は `ByteRange.validate()` を満たす

`ArtifactPushChunkHeader`:

- `deploy_id`
- `offset`
- `length`
- `chunk_sha256`
- `upload_token`

`Validate`:

- `deploy_id`, `chunk_sha256`, `upload_token` 非空
- `length > 0`

`ArtifactPushAck`:

- `received_ranges`
- `next_missing_range`
- `accepted_bytes`

`Validate`:

- 含まれる `ByteRange` をすべて検証

`ArtifactCommitRequest`:

- `deploy_id`
- `artifact_digest`
- `artifact_size`
- `manifest_digest`

`Validate`:

- `deploy_id`, `artifact_digest`, `manifest_digest` 非空
- `artifact_size > 0`

`ArtifactCommitResponse`:

- `artifact_id`
- `verified`

`Validate`:

- `artifact_id` 非空

### 6.4 command start / payload

`CommandStartRequest`:

- `request_id: Uuid`
- `command_type: CommandType`
- `payload: CommandPayload`

`Validate`:

- `request_id` nil UUID 禁止
- `command_type` と `payload` の型一致を必須化

`CommandPayload` は untagged enum:

- `Deploy(DeployCommandPayload)`
- `Run(RunCommandPayload)`
- `Stop(StopCommandPayload)`

`DeployCommandPayload`:

- `deploy_id`
- `expected_current_release`
- `restart_policy`
- `auto_rollback`（default `true`）

`restart_policy` の運用値:

- `never`
- `on-failure`
- `always`
- `unless-stopped`

`RunCommandPayload`:

- `name`

`StopCommandPayload`:

- `name`
- `force`

`DeployCommandPayload` / `RunCommandPayload` / `StopCommandPayload` は
`#[serde(deny_unknown_fields)]` を付与し、未知キーを拒否する。

### 6.5 command event / state / cancel

`CommandEvent`:

- `event_type`
- `request_id: Uuid`
- `command_type`
- `timestamp`
- `stage: Option<String>`
- `error: Option<StructuredError>`

`Validate`:

- `request_id` nil UUID 禁止
- `timestamp` 非空
- `event_type=progress` のとき `stage` 必須かつ非空
- `event_type=failed` のとき `error` 必須かつ `StructuredError.validate()` 成功必須

`StateRequest`:

- `request_id: Uuid`

`Validate`:

- `request_id` nil UUID 禁止

`StateResponse`:

- `request_id: Uuid`
- `state: CommandState`
- `stage: String`
- `updated_at: String`

`Validate`:

- `request_id` nil UUID 禁止
- `state` は `accepted` / `running` のみ許可
- `stage`, `updated_at` 非空

`CommandCancelRequest`:

- `request_id: Uuid`

`Validate`:

- `request_id` nil UUID 禁止

`CommandCancelResponse`:

- `cancellable: bool`
- `final_state: CommandState`

### 6.6 logs

`LogRequest`:

- `process_id: Option<String>`
- `follow: bool`
- `tail_lines: u32`

`Validate`:

- `process_id=Some` の場合のみ非空文字列を必須化
- `tail_lines=0` は許可（snapshot なし）

`LogStreamKind`:

- `stdout`
- `stderr`
- `composite`

`LogChunk`:

- `request_id: Uuid`
- `seq: u64`
- `process_id: String`
- `stream_kind: LogStreamKind`
- `bytes: Vec<u8>`
- `is_last: bool`

`Validate`:

- `request_id` nil UUID 禁止
- `process_id` 非空

`LogErrorCode`:

- `process_not_found`
- `process_not_running`
- `permission_denied`
- `internal`

`LogError`:

- `code: LogErrorCode`
- `message: String`

`Validate`:

- `message` 非空

`LogEnd`:

- `request_id: Uuid`
- `seq: u64`
- `error: Option<LogError>`

`Validate`:

- `request_id` nil UUID 禁止
- `error` がある場合は `LogError.validate()` 成功必須

## 7. Validate 基盤（`validate.rs`）

`ValidationError`:

- `field: &'static str`
- `message: &'static str`

補助関数:

- `ensure_non_empty`
- `ensure_uuid_not_nil`
- `ensure_positive_u64`

設計意図:

- 各型の `Validate` 実装は薄いラッパーに保ち、エラー形を統一する。

## 8. テスト索引

主なテスト群:

- `messages.rs`

## 実装反映ノート（Issue #31 / 2026-02-13）

- `messages.rs` に logs 用 payload 型を追加し、`lib.rs` で再 export した。
- logs payload は DATAGRAM 前提のため `seq` を明示し、欠損検知のみ可能な設計を採用した。
  - `hello_negotiate_round_trip_and_validate`
  - `command_start_rejects_payload_command_mismatch`
  - `state_response_rejects_terminal_states`
  - `command_event_enforces_progress_and_failed_requirements`
- `envelope.rs`
  - `envelope_requires_non_nil_identifiers`
- `error.rs`
  - `rejects_unknown_error_code`
- `cbor.rs`
  - `decode_invalid_bytes_returns_error`

この索引は回帰観点を素早く引くための入口とする。

## 9. 変更時の更新指針

`imago-protocol` の型や `Validate` 契約を変更した場合は、最低限以下を同時更新する。

- [`imago-protocol.md`](./imago-protocol.md)
- [`deploy-protocol.md`](./deploy-protocol.md)
- [`observability.md`](./observability.md)
- crate 内テスト（追加または既存修正）
