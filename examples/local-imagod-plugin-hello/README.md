# local-imagod-plugin-hello example

同一マシン上で `imagod` を起動し、`warg://sizumita:ferris@0.1.0` の component を `wit` source として解決して
`sizumita:ferris/says.say` を呼び出す最小 example です。

## 事前条件

- Rust toolchain（`rustc 1.90` 以上）
- `wasm32-wasip2` target（未導入なら `rustup target add wasm32-wasip2`）
- （任意）鍵を再生成する場合は `cargo run -p imago-cli -- certs generate ...` が実行できること

## ディレクトリ構成

- `imago.toml`: plugin 依存（`[[dependencies]]`）と capability、build/deploy 設定（`wit = "warg://sizumita:ferris@0.1.0"`、`component` 指定なし）
  - `privileged = false` を維持したまま `capabilities.wasi` で必要な WASI interface を明示許可しています。
- `imagod.toml`: ローカル `imagod` 設定
- `Cargo.toml`, `src/main.rs`: `wasi:cli/run` 実装 + `wit-bindgen` で plugin import 呼び出し
- `wit/world.wit`: app world（`sizumita:ferris/says` import）
- `scripts/generate-certs.sh`: ローカル RPK 鍵生成 + `client.pub.hex` 生成
- `scripts/run-imagod.sh`: ローカル `imagod` 起動
- `scripts/deploy.sh`: deploy 実行（内部で build も実行）
- `scripts/verify-hello.sh`: `imago logs` で hello 出力を検証

## 注意（破壊的変更）

- 旧 mTLS/X.509 設定（`ca_cert` / `client_cert` / `tls.server_cert` / `tls.client_ca_cert`）はこの example では利用しません。
- `imago.toml` は `target.default.client_key` のみを使い、サーバ鍵検証は TOFU + `~/.imago/known_hosts`（CLI 既定パス）で行います。

## 手順

1. RPK 鍵を生成

```bash
cd examples/local-imagod-plugin-hello
./scripts/generate-certs.sh
```

このスクリプトは `certs/server.key`・`certs/client.key`・`certs/server.pub.hex`・`certs/client.pub.hex` を再生成し、`imagod.toml` の `tls.client_public_keys` を `client.pub.hex` に同期します。`known_hosts` は example 配下には作成せず、CLI 既定の `~/.imago/known_hosts` に初回接続時 TOFU でサーバ公開鍵が記録され、以後は不一致鍵を拒否します。

2. `imago update` で依存 WIT を `wit/deps/` に展開し、`wit` が component の場合は component hash/source も `imago.lock` へ固定

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

5. ferris 呼び出し出力を検証

```bash
cd examples/local-imagod-plugin-hello
./scripts/verify-hello.sh
```

成功時は logs に `sizumita:ferris` 呼び出しメッセージが含まれます。plugin component 本体は `imago update` で `.imago/deps/` に保存され、`imago build` / `imago deploy` はこの依存キャッシュを使って artifact を構築します。

## 補足

- plugin component / WIT の出所は `sizumita:ferris@0.1.0` です。
- capability ルールは version 付き interface 名（`sizumita:ferris/says@0.1.0.say`）を許可しています。
- runtime は WASI import も deny-by-default なので、`println!` を使うこの example では `capabilities.wasi` の許可が必須です。
- `kind="wasm"` でも `component` を省略できます（`wit` source が component の場合のみ）。`imago update` が lock に `component_*` を自動固定します。
