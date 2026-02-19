# local-imagod example

同一マシン上で `imagod` を起動し、`imago deploy` を実行する最小 example です。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）が使えること
- `wasm32-wasip2` target が入っていること  
  （未導入なら `rustup target add wasm32-wasip2`）
- （任意）鍵を再生成する場合は `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: build/deploy 設定（`[build].command` で直下 `Cargo.toml` を `cargo build --target wasm32-wasip2 --release` 実行し、`main` は `target/.../*.wasm` を参照）
- `imagod.toml`: `imagod` が読むサーバ設定
- `Cargo.toml`, `src/`: 配置対象の最小 CLI Wasm アプリ
- `assets/`: bundle に含めるサンプル asset
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）

## 注意（破壊的変更）

- 旧 mTLS/X.509 設定（`ca_cert` / `client_cert` / `tls.server_cert` / `tls.client_ca_cert`）はこの example では利用しません。
- `imago.toml` は `target.default.client_key` のみを使い、サーバ鍵検証は TOFU + `~/.imago/known_hosts`（CLI 既定パス）で行います。

## 手順

1. `imagod` を起動（ターミナル1）

```bash
cd examples/local-imagod
./scripts/run-imagod.sh
```

2. deploy を実行（ターミナル2）

```bash
cd examples/local-imagod
./scripts/deploy.sh
```

成功時は `command.event` の終端が `succeeded` になり、`imagod` 側にも deploy 成功ログが出ます。
