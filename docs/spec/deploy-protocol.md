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
- Auth: RPK（client key 認証 + server key pin/TOFU）
- Payload format: CBOR
- Rust 実装: `quinn` + `web-transport-quinn`
- `imago deploy` は接続確立フェーズで鍵認証失敗（クライアント鍵拒否、known_hosts 不一致など）を `E_UNAUTHORIZED` として報告する（stage: `transport.connect`）。

### 2.1 ストリーム上のフレーミング

現行実装では、stream 内メッセージを次のフレーム形式で送る。

- `4byte big-endian length`
- `CBOR payload`

運用ルール:

- request は **1 stream あたり 1 envelope のみ**許可する。
- `command.start` は同一 stream 上で `command.start response` と `command.event*` を返す（response/event は複数可）。
- `command.start` が受理前に失敗した場合（payload 検証・認可・shutdown 判定など）は、`type=command.start` の error envelope を 1 件返して stream close する。
- request envelope が複数ある stream は `E_BAD_REQUEST` で拒否する。

### 2.2 DATAGRAM 設定

- `logs` 本文転送は QUIC DATAGRAM を使用する。
- サーバ/クライアントとも `quinn::TransportConfig` で DATAGRAM バッファを明示設定する。
  - `datagram_send_buffer_size = 1MiB`
  - `datagram_receive_buffer_size = Some(1MiB)`
- サーバ/クライアントとも QUIC keepalive / idle timeout を有効化する。
  - `keep_alive_interval = 5s`
  - `max_idle_timeout = 180s`
  - server は `imagod.toml` の `runtime.transport_keepalive_interval_secs` と `runtime.transport_max_idle_timeout_secs` で上書きできる。
- `logs.chunk` payload は `Session::max_datagram_size()` と実装上限（目安 1024 bytes）を同時に満たすサイズで分割送信する。

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

`required_features` 運用方針:

- `imago ps` / `imago compose ps` は `required_features` に `services.list` を含める。
- `hello.negotiate` response の `features` に `services.list` が存在しない場合、CLI は `ps` 系コマンドを実行しない。

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
- `idempotency_key` は `name/type/target/policy/artifact_*/manifest_digest` の canonical 表現を `sha256` した安定キー（`deploy:<hex64>`）を使う。
- upload フェーズ（`hello.negotiate` / `deploy.prepare` / `artifact.push` / `artifact.commit`）は固定回数の自動再試行を行い、再接続後は同一 `idempotency_key` と `missing_ranges` に基づいて再開転送する。

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
- `restart_policy` は `never` / `on-failure` / `always` / `unless-stopped` を受理する。
- 上記以外の値は `E_BAD_REQUEST`。
- manager 起動時の自動復元対象は `restart_policy="always"` の service のみ（best-effort）。
- `on-failure` / `unless-stopped` の高度な再起動戦略は現行未実装（値の受理・保存のみ）。

response:

- `accepted`（bool）
- 受理前失敗時は `accepted` を返さず、`type=command.start` かつ `error` を持つ envelope を返す（`payload=null`）。

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

### 5.9 `logs.request` / `logs.chunk` / `logs.end`

`logs.request` request:

- `name: Option<String>`
- `follow: bool`
- `tail_lines: u32`

制約:

- `name=None` は「リクエスト時点で running のサービス + retained logs が残る停止済みサービス」を対象とする。
- retained logs は imagod プロセス寿命内メモリの global ring にのみ保持し、eviction または imagod 再起動後は参照不可とする。
- 全サービス購読では `tail_lines` は各サービス単位で適用する。
- 受理前エラー（対象未存在・未起動など）は stream 応答の `error` で返す。

`logs.chunk` datagram payload:

- `request_id`
- `seq`
- `name`
- `stream_kind` (`stdout` / `stderr` / `composite`)
- `bytes`
- `is_last`

`logs.end` datagram payload:

- `request_id`
- `seq`
- `error`（任意）

運用ルール:

- `logs.request` 自体は stream で ACK を返し、ログ本文は DATAGRAM のみで送る。
- `seq` は欠損検知専用であり、再送制御は行わない。
- follow 配信で内部購読が `Lagged` した場合、サーバは `seq` を前進させて欠損をクライアントへ通知する（欠落 chunk は送られない）。
- `follow=false` は `logs.end`（または `is_last`）で終端する。
- `follow=true` は明示中断または配信側終了時に `logs.end` を受けて終端する。
- 停止済みサービスに `follow=true` を指定した場合は snapshot 送信後に `logs.end` を返して即終端する。

### 5.10 `rpc.invoke`

request:

- `interface_id`
- `function`
- `args_cbor`（CBOR bytes）
- `target_service.name`

response:

- `result_cbor`（成功時）
- `error`（失敗時、`code` / `stage` / `message`）

制約:

- `result_cbor` と `error` は同時に存在してはならない。
- `tls.client_public_keys` で認証された接続は `rpc.invoke` を許可し、それ以外の管理メッセージは拒否する。
- `type="rpc"` service の runner は起動時に `manifest.main` を自動実行せず、`rpc.invoke` 到着時のみ対象関数を実行する。
- 例: `examples/rpc.invoke.request.json` / `examples/rpc.invoke.response.success.json` / `examples/rpc.invoke.response.error.json`

### 5.11 `services.list`

`services.list` request:

- `names: Option<Vec<String>>`

`services.list` response:

- `services`
  - `name`
  - `state`（`running` / `stopping` / `stopped`）
  - `release_hash`
  - `started_at`（`state=stopped` で unknown の場合は空文字を許容）

制約:

- `names=None` は service 一覧を返す。
- `names=Some([...])` は指定名の一致集合のみ返す。
- `names` に unknown service 名が含まれていてもエラーにしない。
- 指定名がすべて unknown の場合は `services=[]` を返す。
- deployed 情報（`active_release`）が未観測でも runtime snapshot がある service は結果に含める。
- `started_at` は `running` / `stopping` では非空必須、`stopped` では空文字（unknown）を許容する。

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

- `imago-cli` の `deploy` 接続フェーズで、トランスポート認証失敗を `E_UNAUTHORIZED` に正規化する。
- 将来の CONNECT 拒否との整合のため、HTTP status `401` / `403` も `E_UNAUTHORIZED` として扱う。
- 対象は CLI のエラー正規化のみで、認証検証位置（transport handshake）および protocol wire 契約は変更しない。

## 実装反映ノート（Issue #71 / 2026-02-11）

- `imago-cli deploy` の `idempotency_key` を `name/type/target/policy/artifact_digest/artifact_size/manifest_digest` 由来の安定ハッシュへ変更した。
- upload フェーズに自動 retry/resume を導入した（最大 4 試行、待機 250ms -> 500ms -> 1s 上限）。
- 非再試行エラー（`E_UNAUTHORIZED`, `E_BAD_REQUEST`, `E_BAD_MANIFEST`, `E_IDEMPOTENCY_CONFLICT`, `E_RANGE_INVALID`, `E_CHUNK_HASH_MISMATCH`, `E_STORAGE_QUOTA`, `E_PRECONDITION_FAILED`）は即時失敗とする。

## 実装反映ノート（Multi-process Runner / 2026-02-11）

- `imagod` の実行アーキテクチャは manager/runner のマルチプロセス構成へ変更した。
- ただし deploy protocol の wire 契約（`MessageType`, payload schema, state/cancel semantics）は変更しない。
- manager-runner / runner-runner 間の IPC は内部実装であり、本仕様の外部互換性に影響しない。

## 実装反映ノート（Crate Split 6+1 / 2026-02-11）

- `imagod` 実装を複数 crate へ分割したが、本仕様の wire 契約は変更しない。
- 変更は `imagod` 内部モジュール境界の再編のみであり、`MessageType` と payload schema は不変。

## 実装反映ノート（Issue #31 / 2026-02-13）

- `imago logs` の本文転送を stream から DATAGRAM へ移し、`logs.request`（stream）+ `logs.chunk`/`logs.end`（DATAGRAM）へ分離した。
- `name=None` を全サービス購読として定義し、対象集合はリクエスト時点で固定した。
- `seq` は欠損検知のみとし、再送や順序補正は導入しない。

## 実装反映ノート（PR #145 follow-up / 2026-02-13）

- follow 配信中に内部 `broadcast` が `Lagged` した場合、欠落を隠蔽しないため `seq` を前進させる挙動を追加した。
- これによりクライアントは `seq` ギャップから `<<logs truncated>>` 警告を表示できる。

## 実装反映ノート（Issue #87 / 2026-02-15）

- `logs.request` のフィルタキーを `name` へ統一した。
- `logs.request.name=None` は `logs.request` 契約に従う（詳細は 5.9）。
- `logs.request` ACK の対象一覧キーを `names` へ統一した。

## 実装反映ノート（CLI request stream timeout/retry / 2026-02-18）

- `imago-cli` の `request_events` / `request_response` は request stream を timeout 付きで実行する。
- `command.start` 以外の request stream では timeout 対象は `open_bi` / stream write / stream read で、既定値は 30 秒。
- deploy 実行時は `hello.negotiate` の `limits.deploy_stream_timeout_secs` を timeout 値に適用する（server 既定は `runtime.deploy_stream_timeout_secs`）。
- `command.start` 以外の request stream 失敗時は固定回数で再試行する（待機 100ms -> 250ms、最大 3 試行）。
- `command.start` は同一 `request_id` の重複実行を避けるため自動再試行しない（single attempt）。
- `command.start` は `open_bi` / stream write には timeout を適用するが、stream read には絶対 timeout を適用しない。
- request stream 失敗時の最終エラーには、最初の失敗理由と最後の失敗理由を併記して一次障害を可視化する。
- `command.start` の stream 失敗時は「command may still be running」を明示し、in-flight operation の終了確認後に再実行する運用を推奨する。

## 実装反映ノート（RPK + TOFU / 2026-02-18）

- [BREAKING] 認証方式を mTLS/X.509 から RPK + TOFU へ移行した。
- [BREAKING] 接続時のサーバ検証は CA チェーンではなく `known_hosts` の鍵 pin を正本にする。
- `known_hosts` 未登録ホストへの初回接続のみ TOFU で登録し、登録済みホストの鍵不一致は `E_UNAUTHORIZED` を返す。

## 実装反映ノート（Network RPC / 2026-02-18）

- `rpc.invoke` を deploy protocol の有効メッセージ種別へ追加した。
- `rpc.invoke` request は `interface_id` / `function` / `args_cbor` / `target_service.name` を持つ。
- `rpc.invoke` response は `result_cbor` または構造化 `error` の排他的表現を返す。
- クライアント鍵ロールを分離し、`tls.client_public_keys` は `hello.negotiate` と `rpc.invoke` のみ許可する。

## 実装反映ノート（RPC resident runner startup / 2026-02-19）

- `type="rpc"` の runner は起動時に `manifest.main` を自動実行しない。
- `rpc.invoke` が到着するまで runner は常駐待機し、関数実行は `rpc.invoke` でのみ開始する。

## 実装反映ノート（Retained logs 契約 / 2026-02-20）

- `logs.request.name=None` は running サービスに加え、retained logs が残る停止済みサービスも対象に含める。
- retained logs は imagod プロセス寿命内メモリの global ring に保持し、eviction または imagod 再起動後は参照できない。
- 停止済みサービスへ `follow=true` を指定した場合は snapshot 後に `logs.end` を返して即終了する。

## 実装反映ノート（services.list / ps feature gate / 2026-02-21）

- `services.list` を追加し、`names` フィルタで service 一覧照会できる契約を明示した。
- `names` に unknown service 名が含まれてもエラーにせず、該当なしは `services=[]` を返す方針を追加した。
- `imago ps` / `imago compose ps` は `hello.negotiate.required_features` で `services.list` を必須化する方針を追加した。

## 実装反映ノート（services.list runtime-only + started_at unknown / 2026-02-21）

- `services.list` は deployed 情報が無くても runtime snapshot がある service を返す契約へ更新した。
- `ServiceStatusEntry.started_at` は `running` / `stopping` で非空必須、`stopped` で空文字（unknown）を許容する契約へ更新した。
