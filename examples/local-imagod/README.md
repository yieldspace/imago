# local-imagod example

同一マシン上で `imagod` を起動し、`imago deploy` を実行する最小 example です。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）が使えること
- `wasm32-wasip2` target が入っていること  
  （未導入なら `rustup target add wasm32-wasip2`）
- `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: build/deploy 設定（`[build].command` で直下 `Cargo.toml` を `cargo build --target wasm32-wasip2 --release` 実行し、`main` は `target/.../*.wasm` を参照）
- `imagod.toml`: `imagod` が読むサーバ設定
- `Cargo.toml`, `src/`: 配置対象の最小 CLI Wasm アプリ
- `assets/`: bundle に含めるサンプル asset
- `scripts/generate-certs.sh`: ローカル mTLS 証明書生成
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）

## 手順

1. 証明書を生成

```bash
cd examples/local-imagod
./scripts/generate-certs.sh
```

2. `imagod` を起動（ターミナル1）

```bash
cd examples/local-imagod
./scripts/run-imagod.sh
```

3. deploy を実行（ターミナル2）

```bash
cd examples/local-imagod
./scripts/deploy.sh
```

成功時は `command.event` の終端が `succeeded` になり、`imagod` 側にも deploy 成功ログが出ます。
