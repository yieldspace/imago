# local-imagod-http example

同一マシン上で `imagod` を起動し、`type=http` の Wasm `incoming-handler` を deploy して、
`curl` で応答確認する最小 example です。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）
- `wasm32-wasip2` target（未導入なら `rustup target add wasm32-wasip2`）
- `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: `type=http` + `[http].port=18080` を持つ build/deploy 設定
- `imagod.toml`: ローカル `imagod` 設定
- `Cargo.toml`, `src/lib.rs`: `wasi:http/incoming-handler` を export する Wasm コンポーネント
- `scripts/generate-certs.sh`: ローカル mTLS 証明書生成
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）
- `scripts/verify-http.sh`: `curl http://127.0.0.1:18080/` 応答検証

## 手順

1. 証明書を生成

```bash
cd examples/local-imagod-http
./scripts/generate-certs.sh
```

2. `imagod` を起動（ターミナル1）

```bash
cd examples/local-imagod-http
./scripts/run-imagod.sh
```

3. deploy を実行（ターミナル2）

```bash
cd examples/local-imagod-http
./scripts/deploy.sh
```

4. HTTP 応答を検証

```bash
cd examples/local-imagod-http
./scripts/verify-http.sh
```

成功時は `ok: hello from local-imagod-http` が表示されます。
