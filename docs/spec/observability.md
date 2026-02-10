# Deploy Observability Specification

## 目的

micro linux（小RAM・フラッシュ書き込み抑制）環境を前提に、`deploy` / `run` / `stop` の実行状態を軽量に追跡する仕様を固定する。

関連仕様:

- 実行開始仕様: [`deploy-protocol.md`](./deploy-protocol.md)

## 前提

- イベントは永続保存しない。
- イベントの再送はしない。
- 順序保証は WebTransport 同一ストリームの受信順のみ。

<a id="identifier-rules"></a>
## 識別子規約

### `request_id`

- 1 コマンド実行に 1 つ。
- `command.start` と `state.request` の共通キーとして使う。

### `correlation_id`

- 複数サービスのログ相関に使う補助識別子。
- 追跡上必須なのは `request_id`。

<a id="command-stream-events"></a>
## Command Stream イベント

クライアントは WebTransport の bidirectional stream を開き、先頭で `command.start` を送信する。サーバは同一ストリームで `command.event` を push する。
メッセージは `4byte BE length + CBOR` のフレームで順次送る。`deploy` / `run` / `stop` は短命オペレーションとして扱い、terminal event までをこのストリームで完結させる。

### `command.start` 必須フィールド

- `request_id`
- `command_type` (`deploy` / `run` / `stop`)
- `payload`

### `command.event` 共通フィールド

- `event_type` (`accepted` / `progress` / `succeeded` / `failed` / `canceled`)
- `request_id`
- `command_type`
- `timestamp`

`progress` では `stage` を必須とする。`stage` の値はコマンドごとに定義する。

### ストリーム終了

- `succeeded` / `failed` / `canceled` を終端イベントとする。
- 終端イベント受信後はクライアントが stream を close する。

<a id="state-query"></a>
## 状態照会

`state.request` / `state.response` で現在状態のみ返す。

### `state.request` 必須フィールド

- `request_id`

### `state.response` 必須フィールド

- `request_id`
- `state`
- `stage`
- `updated_at`

`state` は実行中状態のみ返す。完了済み・未存在の `request_id` は `E_NOT_FOUND`。
バックグラウンドで継続する Wasm サービスの稼働状態は command stream では追跡しない。
terminal event 送信後の operation 状態は保持しない。

<a id="disconnect-handling"></a>
## 切断時の扱い

- ストリーム切断時、欠落イベントの補填はしない。
- クライアントは必要なら新しい stream で `state.request` を送って現在状態を確認する。

## 異常系

- 不正な `command_type` は `E_BAD_REQUEST`。
- 必須フィールド欠落は `E_BAD_REQUEST`。
- 実行中でない `request_id` への `state.request` は `E_NOT_FOUND`。
- 実行中でない `request_id` への `command.cancel` は `E_NOT_FOUND`。
- `command.cancel` は起動前（spawn 前）のみ有効で、起動後は `cancellable=false`。

## 実装ノート

- RAM 内の一時状態だけで動くことを優先する。
- イベント本体に secret を含めない。
- 現行 CLI では 1 実行につき 1 WebTransport セッションを作成し、実行完了で閉じる。
- 将来は 1 セッション内で複数 stream を並列利用してよい。

## 実装反映ノート（Milestone Phase 1 / 2026-02-10）

- `command.event` は `event_type=progress` のとき `stage` を必須、`event_type=failed` のとき `error` を必須として検証する。
- `state.request` / `command.cancel` の `request_id` は UUID として扱い、nil UUID を拒否する。
- `state.response` は `stage` と `updated_at` の空文字を拒否する。
- `command.event.timestamp` は現時点で文字列として保持し、形式の厳密検証は行わない。
- 完了済みまたは未存在の `request_id` に対する `E_NOT_FOUND` は、`state.request` の試験観点として扱う（issue #130 と整合）。
