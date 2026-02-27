# local-imagod-plugin-native-experimental-gpio example

## 目的

同一マシンで native plugin（`imago:experimental-gpio@0.1.0`）を使い、`delay.delay-ms` と basic digital 操作の呼び出し動作を確認するサンプルです。

## GPIO カタログ設定

digital pin の定義は `imago.toml` の `[resources.gpio]` 配下 `digital_pins` から読み込まれます。  
`resources.gpio` が未設定の場合は空カタログ扱いになり、`get-digital-*` は `undefined-pin-label` を返します。
`digital_pins` では `label` と `value_path` の重複は許可されず、重複時は起動時に設定エラーになります。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

1. ターミナル A で `imagod` を起動します。

```bash
cd examples/local-imagod-plugin-native-experimental-gpio
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

- `experimental-gpio delay-ms completed: 5`

## 任意: digital smoke test

`get-digital-out` も試す場合は deploy 前に次を設定します。

```bash
export IMAGO_EXPERIMENTAL_GPIO_TRY_DIGITAL=1
```

GPIO backend の実体ファイルが利用できない環境では `digital smoke test failed` が出る想定です。
