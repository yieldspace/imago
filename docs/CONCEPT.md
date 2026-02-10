# Concept: 理念から仕様へ

imago は「組込み開発の敷居を下げる」ことを目的に、Wasm Component を配布単位として扱う実行・配布基盤である。

## 理念

1. 同一 Wasm を複数環境で動かせること。
2. 権限を明示し、安全境界を仕様で固定すること。
3. リモート配布を 1 コマンドで実行できること。

## 実装方針

### 実行モデル

- `cli` / `http` / `socket` の 3 モデルを提供する。
- 設定は `imago.toml` を唯一入力とし、deploy 時に正規化する。

詳細: [`docs/spec/config.md`](./spec/config.md)

### 配布モデル

- build 結果は `build/manifest.json` を中心に扱う。
- 送信整合性は SHA-256 で検証する。
- secret は deploy payload に同梱し、出力経路ではマスクする。

詳細: [`docs/spec/manifest.md`](./spec/manifest.md)

### 通信モデル

- CLI と daemon は QUIC + WebTransport + CBOR で通信する。
- mTLS で相互認証する。
- deploy は再送可能アップロードと非同期 operation で実行する。

詳細: [`docs/spec/deploy-protocol.md`](./spec/deploy-protocol.md)

### 観測モデル

- operation は切断後も追跡できる。
- watch は型付きイベントで状態遷移を返す。
- 履歴は 24 時間保持し、cursor で再取得する。

詳細: [`docs/spec/observability.md`](./spec/observability.md)

## MVP スコープ

- NanoKVM を優先ターゲットにする。
- Syslog 受信・保存・転送を成立させる。
- NanoKVM キャプチャ取得と Discord 送信を成立させる。

計画全体: [`docs/MVP_PLAN.md`](./MVP_PLAN.md)
