# local-imagod-socket example

同一マシン上で `imagod` を起動し、`type=socket` の UDP echo アプリを deploy する example です。

このアプリは `tokio::runtime::Builder::new_current_thread()` を使って current-thread runtime を構築し、
nonblocking UDP 受信キューを反復的に drain して、受信した datagram をすべて送信元へ echo します。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）
- `wasm32-wasip2` target（未導入なら `rustup target add wasm32-wasip2`）
- （任意）鍵を再生成する場合は `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: `type=socket` + `[socket]` 設定（`udp` / `both` / `0.0.0.0:5000`）
- `imagod.toml`: ローカル `imagod` 設定
- `Cargo.toml`, `src/main.rs`: tokio current-thread runtime + UDP echo 実装
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）

## 注意（破壊的変更）

- 旧 mTLS/X.509 設定（`ca_cert` / `client_cert` / `tls.server_cert` / `tls.client_ca_cert`）はこの example では利用しません。
- `imago.toml` は `target.default.client_key` のみを使い、サーバ鍵検証は TOFU + `~/.imago/known_hosts`（CLI 既定パス）で行います。

## 手順

1. `imagod` を起動（ターミナル1）

```bash
cd examples/local-imagod-socket
./scripts/run-imagod.sh
```

2. deploy を実行（ターミナル2）

```bash
cd examples/local-imagod-socket
./scripts/deploy.sh
```

3. UDP echo を確認（ターミナル3）

```bash
printf "hello-udp\n" | nc -u -w 1 127.0.0.1 5000
```

`imagod` 側ログに receive/send が出力され、同じ payload が返れば成功です。
