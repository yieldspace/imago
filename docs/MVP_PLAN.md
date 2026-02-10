# MVP計画（NanoKVM）

imago の MVP は、NanoKVM 上で Wasm コンポーネントを安全に配布・実行できる状態を作ることを目的とする。

詳細仕様の正本は [`docs/spec/README.md`](./spec/README.md)。

## MVP の到達点

- `imago deploy` で build から実行追跡まで完結する。
- Syslog 受信と一時保存、外部転送まで動く。
- NanoKVM キャプチャを取得し Discord Webhook に送信できる。
- `run` / `stop` / `logs` / `ps` で運用できる。

## 実装対象

### 設定と権限

- `imago.toml` の必須キーと上書き規則
- deny-by-default の capabilities
- `privileged` の全許可挙動

詳細: [`docs/spec/config.md`](./spec/config.md)

### build と manifest

- `build/manifest.json` の固定フォーマット
- hash 対象の固定
- secret 同梱方針

詳細: [`docs/spec/manifest.md`](./spec/manifest.md)

### deploy 通信

- QUIC + WebTransport + CBOR
- mTLS
- prepare / push / commit / execute / watch
- 冪等性、CAS、自動ロールバック

詳細: [`docs/spec/deploy-protocol.md`](./spec/deploy-protocol.md)

### operation 追跡

- `request_id` / `correlation_id`
- 型付き watch イベント
- 24h 保持、cursor 再取得

詳細: [`docs/spec/observability.md`](./spec/observability.md)

## 受け入れ観点

1. `docs/spec` だけで実装判断が可能である。
2. deploy の中断復帰と再接続追跡ができる。
3. 失敗時に rollback 結果を識別できる。
4. Syslog と capture の 2 ユースケースが再現できる。

## 非対象

- blue-green
- 差分配信
- 監視ダッシュボード
- メトリクスの詳細設計
