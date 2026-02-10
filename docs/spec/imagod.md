# imagod Server Specification

## 目的

`imagod` は deploy protocol のサーバ実装であり、CLI からの artifact 受領・配置・Wasm 実行を担う。

## 通信スタック

- QUIC 実装: `quinn`
- WebTransport: `web-transport-quinn`
- メッセージ: CBOR
- 認証: mTLS

## サーバ設定（`imagod.toml`）

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
```

## メッセージ運用

- 1 stream 上で length-prefix（4byte BE） + CBOR を複数フレーム送信できる。
- `command.start` は同一 stream で `command.start response` の後に `command.event*` を push する。

## 配置・起動

1. `deploy.prepare` で upload session を確立
2. `artifact.push` で chunk を受信（sha256 検証）
3. `artifact.commit` で全体 digest 検証
4. `command.start (deploy)` で artifact 展開
5. `/etc/imago/services/<name>/<hash>/` 配下へ配置
6. 旧版を起動前 cleanup
7. Wasmtime (`wasi:cli/run`) で component を実行

## ロールバック

- `auto_rollback=true` かつ起動失敗時は直前 active release へ戻す。
- 巻き戻し失敗時は `E_ROLLBACK_FAILED`。

## 状態追跡

- 状態遷移: `accepted -> running -> succeeded|failed|canceled`
- `state.request` は実行中のみ返却
- 完了済みは `E_NOT_FOUND`
