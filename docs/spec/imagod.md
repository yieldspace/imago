# imagod Server Specification (Overview)

## 目的

`imagod` は deploy protocol のサーバ実装であり、`imago-cli` からの要求を受けて、artifact 受領・配置・Wasm 実行管理を担う。

このページは**概要仕様**のみを扱う。内部構造の正本は [`imagod-internals.md`](./imagod-internals.md) とする。

## 責務境界

- 通信終端: QUIC + WebTransport + CBOR メッセージ処理
- 認証: mTLS（クライアント証明書必須）
- deploy 実行: artifact upload/commit、release 展開、サービス起動
- 実行管理: `run` / `stop`、同名サービス置換、終了監視
- 状態追跡: `command.event` / `state.request` / `command.cancel`

以下は扱わない（または未実装）。

- blue-green デプロイ
- イベント永続化と再送
- restart policy の高度化
- 再起動跨ぎの service 状態復元

## 外部仕様への参照

- 通信仕様: [`deploy-protocol.md`](./deploy-protocol.md)
- 観測仕様: [`observability.md`](./observability.md)
- 設定仕様: [`config.md`](./config.md)
- manifest 仕様: [`manifest.md`](./manifest.md)

## 設定サマリー（`imagod.toml`）

```toml
listen_addr = "[::]:4443"
storage_root = "/etc/imago"
server_version = "imagod/0.1.0"
compatibility_date = "2026-02-10"

[tls]
server_cert = "/etc/imago/certs/server.crt"
server_key = "/etc/imago/certs/server.key"
client_ca_cert = "/etc/imago/certs/ca.crt"

[runtime]
chunk_size = 1048576
max_inflight_chunks = 16
upload_session_ttl_secs = 900
stop_grace_timeout_secs = 30
epoch_tick_interval_ms = 50
```

詳細なバリデーション条件と既定値の意味は [`config.md`](./config.md) を参照。

## 実装追従方針

- この概要ページは「責務境界」と「外部仕様の参照点」を維持する。
- 内部挙動はコード断片ではなく、**ファイルパス + 構造体/関数名**で追跡する。
- 内部挙動変更時は、必ず [`imagod-internals.md`](./imagod-internals.md) を更新する。
