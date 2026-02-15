# local-imagod-socket example

同一マシン上で `imagod` を起動し、`type=socket` の UDP echo アプリを deploy する example です。

このアプリは `tokio::runtime::Builder::new_current_thread()` を使って current-thread runtime を構築し、
nonblocking UDP 受信キューを反復的に drain して、受信した datagram をすべて送信元へ echo します。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）
- `wasm32-wasip2` target（未導入なら `rustup target add wasm32-wasip2`）
- `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: `type=socket` + `[socket]` 設定（`udp` / `both` / `0.0.0.0:5000`）
- `imagod.toml`: ローカル `imagod` 設定
- `Cargo.toml`, `src/main.rs`: tokio current-thread runtime + UDP echo 実装
- `scripts/generate-certs.sh`: ローカル mTLS 証明書生成
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）

## 手順

1. 証明書を生成

```bash
cd examples/local-imagod-socket
./scripts/generate-certs.sh
```

2. `imagod` を起動（ターミナル1）

```bash
cd examples/local-imagod-socket
./scripts/run-imagod.sh
```

3. deploy を実行（ターミナル2）

```bash
cd examples/local-imagod-socket
./scripts/deploy.sh
```

4. UDP echo を確認（ターミナル3）

```bash
printf "hello-udp\n" | nc -u -w 1 127.0.0.1 5000
```

`imagod` 側ログに receive/send が出力され、同じ payload が返れば成功です。
