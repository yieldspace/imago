# Deploy Observability Specification

## 目的

長時間 operation、接続断、再接続、失敗復旧時に必要な追跡契約を固定する。

関連仕様:

- 実行開始契約: [`deploy-protocol.md`](./deploy-protocol.md)

<a id="identifier-rules"></a>
## 識別子規約

### `request_id`

- 1 リクエストに 1 つ。
- 冪等性判定の入力には使わない。

### `correlation_id`

- 同一 deploy / operation を横断して追跡する ID。
- `operation.watch` イベントに必ず含める。

<a id="operation-snapshot"></a>
## operation スナップショット

`operation.get` は以下を返す。

- `operation_id`
- `state` (`accepted` / `running` / `succeeded` / `failed` / `rolled_back`)
- `stage` (`validate` / `expand` / `cleanup` / `start` / `rollback`)
- `progress` (0-100)
- `release_id` (任意)
- `process_id` (任意)
- `error` (失敗時)
- `updated_at`

<a id="operation-watch-events"></a>
## `operation.watch` イベント

`operation.watch` は型付きイベントを返す。各イベントは以下共通フィールドを持つ。

- `event_type`
- `operation_id`
- `sequence`
- `timestamp`
- `correlation_id`

### イベント種別

1. `operation.accepted`
2. `operation.progress`
3. `operation.succeeded`
4. `operation.failed`
5. `operation.rollback_started`
6. `operation.rollback_succeeded`
7. `operation.rollback_failed`

### 種別ごとの追加必須フィールド

- `operation.progress`: `stage`, `progress`
- `operation.failed`: `stage`, `error`
- `operation.rollback_started`: `from_stage`
- `operation.rollback_failed`: `error`

<a id="retention-and-range"></a>
## 保持期間と取得範囲

- operation 履歴保持期間は 24 時間。
- 再取得は `cursor` + `limit` 方式。
- `limit` の既定値は `1000`。
- 返却順は `sequence` 昇順。
- 取得上限到達時は `next_cursor` を返す。

## 再接続時の期待動作

1. クライアントは最後に受信した `sequence` に対応する `cursor` を保存する。
2. 再接続後、`operation.watch` を `cursor` 指定で再開する。
3. サーバは欠落イベントを順に返し、最後に最新状態へ追従させる。

## 異常系

- 保持期間外の `cursor` は `E_NOT_FOUND`。
- 不正 `cursor` は `E_BAD_REQUEST`。
- `limit` が上限超過時はサーバ側上限に丸める。

## 実装ノート

- イベント本体に secret を含めない。
- `operation.get` と `operation.watch` の最終状態は一致する必要がある。
