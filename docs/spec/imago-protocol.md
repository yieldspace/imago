# imago-protocol Specification (Overview)

## 目的

`imago-protocol` は `imago-cli` と `imagod` が共有する wire 契約を Rust 型として定義する crate である。
この文書は公開インターフェースの概要を示す。詳細実装は [`imago-protocol-internals.md`](./imago-protocol-internals.md) を正本とする。

## 責務境界

`imago-protocol` が担う責務:

- CBOR エンコード/デコード API（`to_cbor` / `from_cbor`）
- 共通封筒型 `ProtocolEnvelope<T>`
- deploy/command/state/cancel 各メッセージの型定義
- 構造化エラー `StructuredError` と `ErrorCode`
- バリデーション契約 `Validate`

`imago-protocol` が担わない責務:

- QUIC/WebTransport の送受信
- メッセージのフレーミング（4byte length 付与など）
- ストレージ/実行/運用ロジック

## 公開 API マップ

| 分類 | 主な公開型/関数 | 役割 |
|---|---|---|
| CBOR | `to_cbor`, `from_cbor`, `CborError` | 型とバイト列の相互変換 |
| 封筒 | `ProtocolEnvelope<T>` | `type`/識別子/`payload`/`error` の統一表現 |
| エラー | `StructuredError`, `ErrorCode` | wire 上の失敗契約 |
| メッセージ | `MessageType` と各 `*Request`/`*Response` | プロトコル payload 契約 |
| 検証 | `Validate`, `ValidationError` | 受信値の事前妥当性検査 |

## 共通識別子方針

- `ProtocolEnvelope.request_id` は UUID（nil UUID 不可）。
- `ProtocolEnvelope.correlation_id` は UUID（nil UUID 不可）。
- `CommandStartRequest.request_id` / `StateRequest.request_id` / `CommandCancelRequest.request_id` も UUID（nil UUID 不可）。

## 互換キー方針

`hello.negotiate` request は `compatibility_date` を用いる。

- 型: `String`
- 意味: 両端点が同じ互換基準日で動作しているかを確認するためのキー
- 現行判定: 文字列完全一致（`imagod` 側実装）

## 重要なメッセージ契約

- `deploy.prepare` は `app_type` フィールドを持ち、wire key は `"type"`。
- `command.start` は `command_type` と `payload` の組み合わせ一致を必須とする。
- `state.request` の応答メッセージ種別は `state.response`。
- `state.response.state` は `accepted`/`running` のみ許可し、terminal state を禁止する。
- `StructuredError.details` は `BTreeMap<String, String>`。

## 関連仕様

- deploy 通信仕様: [`deploy-protocol.md`](./deploy-protocol.md)
- 観測仕様: [`observability.md`](./observability.md)
- `imagod` 概要: [`imagod.md`](./imagod.md)
- `imago-protocol` 内部詳細: [`imago-protocol-internals.md`](./imago-protocol-internals.md)
