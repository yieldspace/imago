# Deploy Observability Specification

## 1. 目的

`deploy` / `run` / `stop` の実行状態を軽量に追跡する契約を固定する。

関連仕様:

- 通信手順: [`deploy-protocol.md`](./deploy-protocol.md)
- 型契約: [`imago-protocol.md`](./imago-protocol.md)

## 2. 前提

- イベントは永続保存しない。
- イベントの再送はしない。
- 順序保証は同一 stream の受信順のみ。
- operation の状態は終端後に削除される。

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

## 実装反映ノート（Milestone Phase 1 / 2026-02-10）

- `state.response` は terminal state を禁止する。
- `request_id` / `correlation_id` は UUID として扱い、nil UUID を拒否する。
- `StructuredError.details` は `BTreeMap<String, String>` として扱う。
