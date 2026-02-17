# local-imagod-plugin-hello example

同一マシン上で `imagod` を起動し、`warg://chikoski:hello-world@0.2.0` の wasm plugin component を使って
`chikoski:hello/greet.hello` を呼び出す最小 example です。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）
- `wasm32-wasip2` target（未導入なら `rustup target add wasm32-wasip2`）
- `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: plugin 依存（`[[dependencies]]`）と capability、build/deploy 設定（`wit = "warg://chikoski:hello@0.1.0"`）
- `imagod.toml`: ローカル `imagod` 設定
- `Cargo.toml`, `src/lib.rs`: `wasi:cli/run` 実装 + `wit-bindgen` で plugin import 呼び出し
- `wit/world.wit`: app world（`chikoski:hello/greet` import）
- `scripts/generate-certs.sh`: ローカル mTLS 証明書生成
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）
- `scripts/verify-hello.sh`: `imago logs` で hello 出力を検証

## 手順

1. 証明書を生成

```bash
cd examples/local-imagod-plugin-hello
./scripts/generate-certs.sh
```

2. `imago update` で依存 WIT を `wit/deps/` に展開し、component hash を `imago.lock` へ固定

```bash
cd examples/local-imagod-plugin-hello
cargo run --manifest-path ../../Cargo.toml -p imago-cli -- update
```

3. `imagod` を起動（ターミナル1）

```bash
cd examples/local-imagod-plugin-hello
./scripts/run-imagod.sh
```

4. deploy を実行（ターミナル2）

```bash
cd examples/local-imagod-plugin-hello
./scripts/deploy.sh
```

5. hello 出力を検証

```bash
cd examples/local-imagod-plugin-hello
./scripts/verify-hello.sh
```

成功時は logs に hello メッセージが含まれます。plugin component 本体は `imago deploy` 実行時に取得され、同一 hash があれば `.imago/components/` の cache が再利用されます。

## 補足

- plugin component の出所は `chikoski:hello-world@0.2.0` です。
- runtime import 解決との整合のため、`imago.toml` の依存 package 名は `chikoski:hello` を使用しています。
