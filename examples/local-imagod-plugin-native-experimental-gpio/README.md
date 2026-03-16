# local-imagod-plugin-native-experimental-gpio example

## 目的

同一マシンで native plugin（`imago:experimental-gpio@0.1.0`）を使い、raw GPIO API を呼び出せることを確認するサンプルです。

## Dependency

この example は app code で raw `imago:experimental-gpio` だけを import します。board 固有 dependency や board resolver は使いません。

`imago.toml` には native `imago:experimental-gpio` だけを宣言します。

app 側の `[capabilities.deps]` も raw GPIO package だけに対して開きます。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。
`imago.toml` の `remote = "ssh://localhost?socket=/tmp/imagod-local-plugin-native-experimental-gpio.sock"` と `imagod.toml` の `control_socket_path` を一致させ、同じユーザーからその socket に接続できる状態にしてください。

1. ターミナル A で `imagod` 起動を行います。

```bash
cd ../../../imago/examples/local-imagod-plugin-native-experimental-gpio
cargo run -p imago-cli -- deps sync
cargo run -p imagod -- --config "$(pwd)/imagod.toml"
```

2. ターミナル B で deploy とログ確認を行います。

```bash
cd examples/local-imagod-plugin-native-experimental-gpio
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-plugin-native-experimental-gpio-app --tail 200
```

## 成功判定

ログに次の文字列が含まれれば成功です。

- `experimental-gpio example started`
- `set IMAGO_EXPERIMENTAL_GPIO_LABEL to run a digital smoke test`

## 任意: digital smoke test

特定の digital label に対して raw GPIO API の smoke test を試す場合は deploy 前に次を設定します。

```bash
export IMAGO_EXPERIMENTAL_GPIO_LABEL=<your-digital-label>
```

この example 自体は board catalog を持たないので、実機 smoke test を通すには対象 label が `resources.gpio` か別の provider で解決できる必要があります。GPIO backend の実体ファイルが利用できない環境では `raw gpio smoke test failed` が出る想定です。
