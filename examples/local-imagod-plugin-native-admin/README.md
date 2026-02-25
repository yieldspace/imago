# local-imagod-plugin-native-admin example

## 目的

同一マシンで native plugin（`imago:admin@0.1.0`）を使い、管理メタデータ取得を確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

1. ターミナル A で `imagod` を起動します。

```bash
cd examples/local-imagod-plugin-native-admin
cargo run -p imago-cli -- update
cargo run -p imagod -- --config "$(pwd)/imagod.toml"
```

2. ターミナル B で deploy とログ確認を行います。

```bash
cd examples/local-imagod-plugin-native-admin
cargo run -p imago-cli -- deploy --target default --detach
cargo run -p imago-cli -- logs local-imagod-plugin-native-admin-app --tail 200
```

## 成功判定

ログに次の文字列が含まれれば成功です。

- `imago-admin service-name=`
- `imago-admin release-hash=`
- `imago-admin runner-id=`
- `imago-admin app-type=cli`
