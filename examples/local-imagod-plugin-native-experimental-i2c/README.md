# local-imagod-plugin-native-experimental-i2c example

## 目的

同一マシンで native plugin（`imago:experimental-i2c@0.1.0`）を使い、`provider.open-delay` と `delay.delay-ns` の動作を確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。
あわせて OpenSSH client/server を用意し、`ssh localhost true` が対話なしで成功し、`imagod proxy-stdio --socket /tmp/imagod-local-plugin-native-experimental-i2c.sock` を SSH ログインシェルから実行できる状態にしてください。

1. ターミナル A で `imagod` を起動します。

```bash
cd examples/local-imagod-plugin-native-experimental-i2c
cargo run -p imago-cli -- deps sync
cargo run -p imagod -- --config "$(pwd)/imagod.toml"
```

2. ターミナル B で deploy とログ確認を行います。

```bash
cd examples/local-imagod-plugin-native-experimental-i2c
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-plugin-native-experimental-i2c-app --tail 200
```

## 成功判定

ログに次の文字列が含まれれば成功です。

- `experimental-i2c delay-ns completed: 5000000`

## 任意: default bus オープン確認

`open-default-i2c` も試す場合は deploy 前に次を設定します。

```bash
export IMAGO_EXPERIMENTAL_I2C_TRY_OPEN_DEFAULT=1
```

Linux 以外では `open-default-i2c failed` が表示される想定です。
