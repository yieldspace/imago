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
stop_grace_timeout_secs = 30
epoch_tick_interval_ms = 50
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
7. Wasmtime async (`wasi:cli/run`) で component をバックグラウンド実行
8. deploy 完了は「spawn 成功」を意味し、component 終了待ちはしない

## サービス管理

- `ServiceSupervisor` が `service_name -> RunningService` を in-memory で管理する。
- 同名の再 deploy は「旧サービス停止（graceful + timeout 強制停止）→ 新サービス起動」。
- `command.start(run)` は active release を読み出して起動する。
- `command.start(stop)` は対象サービスを停止する。
- 再起動ポリシーは MVP では `never`（未実装）。

## ロールバック

- `auto_rollback=true` かつ起動失敗時は直前 active release へ戻し、再起動を試みる。
- 巻き戻し失敗時は `E_ROLLBACK_FAILED`。

## 状態追跡

- 状態遷移: `accepted -> running -> succeeded|failed|canceled`
- `state.request` は実行中のみ返却
- 完了済みは `E_NOT_FOUND`
- `command.cancel` は起動前のみ有効（起動後は `cancellable=false`）
