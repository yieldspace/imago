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
- RPK + TOFU で相互認証する（初回接続で known_hosts へ pin）。
- deploy/run/stop は command stream を開いて実行する。

詳細: [`docs/spec/deploy-protocol.md`](./spec/deploy-protocol.md)

### 観測モデル

- サーバは command stream 上でイベントを push する。
- 実行中の現在状態は `state.request/state.response` で照会する。
- micro linux 前提としてイベントの永続保存と再送は行わない。

詳細: [`docs/spec/observability.md`](./spec/observability.md)

## MVP スコープ

- NanoKVM を優先ターゲットにする。
- Syslog 受信・保存・転送を成立させる。
- NanoKVM キャプチャ取得と Discord 送信を成立させる。

計画全体: [`docs/MVP_PLAN.md`](./MVP_PLAN.md)

## 実装反映ノート（RPK + TOFU / 2026-02-18）

- [BREAKING] 通信認証モデルを mTLS/X.509 から RPK + TOFU へ置換した。
- サーバ認証は CA チェーンではなく `known_hosts` への鍵 pin を正本とする。
- クライアント認証は `client_key` とサーバ側 `client_public_keys` allowlist で行う。
