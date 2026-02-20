# local-imagod-plugin-native-admin example

## 目的

同一マシンで native plugin（`imago:admin@0.1.0`）を使い、管理メタデータ取得を確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

```bash
cd examples/local-imagod-plugin-native-admin
cargo run --manifest-path ../../Cargo.toml -p imago-cli -- update
./scripts/run-imagod.sh
# 別ターミナル
cd examples/local-imagod-plugin-native-admin
./scripts/deploy.sh
./scripts/verify-admin.sh
```

## 成功判定

`./scripts/verify-admin.sh` で `imago-admin service-name=` などの管理情報が確認できれば成功です。
