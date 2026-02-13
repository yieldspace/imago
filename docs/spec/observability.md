# Deploy Observability Specification

## 1. 目的

`deploy` / `run` / `stop` の実行状態と `logs` の配信挙動を軽量に追跡する契約を固定する。

関連仕様:

- 通信手順: [`deploy-protocol.md`](./deploy-protocol.md)
- 型契約: [`imago-protocol.md`](./imago-protocol.md)

## 2. 前提

- イベントは永続保存しない。
- イベントの再送はしない。
- 順序保証は同一 stream の受信順のみ。
- operation の状態は終端後に削除される。
- `logs` 本文配信は DATAGRAM を使い、欠損/順不同を許容する。

## 3. 識別子規約

### 3.1 `request_id`

- 1 コマンド実行を識別する主キー。
- UUID を使う（nil UUID 不可）。
- `command.start` / `command.event` / `state.request` / `command.cancel` で共通に扱う。

### 3.2 `correlation_id`

- 複数ストリーム横断のログ相関に使う補助キー。
- UUID を使う（nil UUID 不可）。

## 4. Command Stream 契約

### 4.1 開始

- クライアントは bi-stream を開き、先頭に `command.start` を送る。
- サーバは同一 stream 上で `command.start response` と `command.event` を返す。

### 4.2 `command.event` 必須条件

共通必須:

- `event_type`
- `request_id`
- `command_type`
- `timestamp`

追加必須:

- `event_type=progress` のとき `stage`
- `event_type=failed` のとき `error`

### 4.3 終端

- `succeeded` / `failed` / `canceled` を終端イベントとする。
- operation は終端イベント送信後に terminal 状態へ更新され、その後削除される。
- 終端イベント送信が失敗した場合でも、operation は終端化して削除される（リーク防止）。

## 5. 状態照会契約

`state.request` / `state.response` は「実行中状態の一点照会」に限定する。

### 5.1 `state.request`

- 入力: `request_id`

### 5.2 `state.response`

- 出力: `request_id`, `state`, `stage`, `updated_at`
- `state` は `accepted` / `running` のみ
- terminal state（`succeeded` / `failed` / `canceled`）は返却不可

operation が存在しない場合の扱い:

- `state.request`: `E_NOT_FOUND`
- `state.request` のエラー応答 envelope `type` は `state.response`

## 6. cancel 契約

### 6.1 `command.cancel`

- 入力: `request_id`
- 出力: `cancellable`, `final_state`

### 6.2 有効境界

- 起動前（spawn 前）は cancel 可能。
- 起動後は cancel 不可。
- 終端済み operation は `E_NOT_FOUND`（非保持に加え、終端直後の非実行状態も含む）。

## 7. 異常系

- UUID nil は `E_BAD_REQUEST`。
- 必須フィールド欠落は `E_BAD_REQUEST`。
- 実行中でない `request_id` への `state.request` / `command.cancel` は `E_NOT_FOUND`。

## 8. 運用ノート

- command stream は短命オペレーションの結果通知に限定する。
- 長期稼働サービス（loop する component など）の稼働監視は supervisor 側で扱う。
- 欠落イベントの補填は行わないため、必要に応じて再照会する。

## 9. Logs DATAGRAM 契約

### 9.1 開始

- クライアントは stream で `logs.request` を送信する。
- サーバは同一 stream で ACK（`logs.request` response）を返し、その後ログ本文は DATAGRAM (`logs.chunk`) で送信する。

### 9.2 payload 契約

- `logs.chunk.seq` は request 単位で単調増加し、欠損検知専用とする。
- follow 配信で内部購読が `Lagged` になった場合、サーバは欠落分の `seq` を前進させて欠損を可視化する（欠落 chunk 自体は再送しない）。
- `logs.chunk.is_last=true` は最終データチャンクを示す。
- `logs.end` は購読終端を示し、配信開始後の異常は `logs.end.error` で返す。

### 9.3 `process_id` と全サービス購読

- `logs.request.process_id=None` は「リクエスト時点で running の全サービス」を意味する。
- 後から起動したサービスは同一購読に自動追加しない。
- 全サービス購読時の `--tail N` は各サービスごとに末尾 N 行を返す。

### 9.4 再接続/再購読

- `logs` は再送や resume offset を持たない。
- 切断後はクライアントが新しい `logs.request` を再発行して再購読する。
- 欠損補填が必要な場合は `--tail` 付きで再接続して直近ログを取り直す。

## 実装反映ノート（Milestone Phase 1 / 2026-02-10）

- `state.response` は terminal state を禁止する。
- `request_id` / `correlation_id` は UUID として扱い、nil UUID を拒否する。
- `StructuredError.details` は `BTreeMap<String, String>` として扱う。

## 実装反映ノート（Issue #31 / 2026-02-13）

- `logs.request` は stream、`logs.chunk` / `logs.end` は DATAGRAM で扱う契約へ拡張した。
- `seq` ギャップは欠損検知のみを目的とし、再送制御は行わない。
- `process_id=None` の全サービス購読は「現在 running のみ・後起動は非対象」と定義した。

## 実装反映ノート（PR #145 follow-up / 2026-02-13）

- follow 配信中に内部 `broadcast` が `Lagged` した場合、サーバは `seq` を前進させてクライアント側の欠損警告を可能にした。
- 欠落ログの再送制御は追加せず、必要時は `--tail` を使った再購読で補う方針を維持する。
