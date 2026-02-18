# local-imagod-plugin-native-admin example

同一マシン上で `imagod` を起動し、`kind="native"` の `imago:admin@0.1.0` plugin から
runner の管理メタデータ（service/release/runner/app-type）を取得する最小 example です。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）
- `wasm32-wasip2` target（未導入なら `rustup target add wasm32-wasip2`）
- `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: native plugin 依存（`[[dependencies]]`）と capability、build/deploy 設定
- `imagod.toml`: ローカル `imagod` 設定
- `Cargo.toml`, `src/main.rs`: `wasi:cli/run` 実装 + `wit-bindgen` で `imago:admin/runtime` 呼び出し
- `wit/world.wit`: app world（`imago:admin/runtime@0.1.0` import）
- `../../plugins/imago-admin/wit/package.wit`: `imago:admin` native plugin 契約（runtime 実装は `plugins/imago-admin` crate）
- `scripts/generate-certs.sh`: ローカル mTLS 証明書生成
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）
- `scripts/verify-admin.sh`: `imago logs` で管理情報出力を検証

## 手順

1. 証明書を生成

```bash
cd examples/local-imagod-plugin-native-admin
./scripts/generate-certs.sh
```

2. `imago update` で依存 WIT を `wit/deps/` に展開して `imago.lock` へ固定

```bash
cd examples/local-imagod-plugin-native-admin
cargo run --manifest-path ../../Cargo.toml -p imago-cli -- update
```

3. `imagod` を起動（ターミナル1）

```bash
cd examples/local-imagod-plugin-native-admin
./scripts/run-imagod.sh
```

4. deploy を実行（ターミナル2）

```bash
cd examples/local-imagod-plugin-native-admin
./scripts/deploy.sh
```

5. `imago:admin` 呼び出し出力を検証

```bash
cd examples/local-imagod-plugin-native-admin
./scripts/verify-admin.sh
```

成功時は logs に下記のような行が含まれます。

- `imago-admin service-name=...`
- `imago-admin release-hash=...`
- `imago-admin runner-id=...`
- `imago-admin app-type=cli`

補足:

- この example は `capabilities.privileged = true` を使い、WASI 許可ルールの列挙を省略しています。
