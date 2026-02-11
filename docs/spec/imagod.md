# imagod Server Specification (Overview)

## 1. 目的

`imagod` は deploy protocol のサーバ実装であり、`imago-cli` からの要求を受けて artifact 受領・配置・Wasm 実行管理を行う。

このページは概要層のみを扱う。内部構造の正本は [`imagod-internals.md`](./imagod-internals.md)。

## 2. 責務境界

`imagod` の責務:

- QUIC + WebTransport セッション受理
- `ProtocolEnvelope` (`MessageType`) の decode/dispatch
- mTLS 認証（クライアント証明書必須）
- `deploy.prepare` / `artifact.push` / `artifact.commit`
- `command.start` (`deploy` / `run` / `stop`) と `command.event` 配信
- `state.request -> state.response` の実行中状態照会
- `command.cancel` の起動前 cancel 判定

`imagod` の非責務（または未実装）:

- イベント永続化・再送
- 再起動跨ぎの service 状態復元
- 高度な restart policy
- blue-green デプロイ

## 3. 外部仕様との対応

- 通信仕様: [`deploy-protocol.md`](./deploy-protocol.md)
- 観測仕様: [`observability.md`](./observability.md)
- 設定仕様: [`config.md`](./config.md)
- protocol 型仕様: [`imago-protocol.md`](./imago-protocol.md)

## 4. 互換キー方針

`hello.negotiate` では `compatibility_date` を使う。

- 既定値: `2026-02-10`
- 判定: 現行は文字列一致
- `protocol_draft` は受理しない

## 5. 設定サマリー（`imagod.toml`）

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

詳細は [`config.md`](./config.md) を参照。

## 6. 実装追従方針

- 概要ページは責務境界と外部契約の橋渡しに限定する。
- 内部挙動は `crates/imagod/src/*` の関数/型名で追跡し、[`imagod-internals.md`](./imagod-internals.md) を更新する。
- `imago-protocol` 側の型・検証契約を変更した場合、`imagod` 側ドキュメントを同時に更新する。
