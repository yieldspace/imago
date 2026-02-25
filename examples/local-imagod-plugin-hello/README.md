# local-imagod-plugin-hello example

## 目的

同一マシンで Wasm plugin（`warg://sizumita:ferris@0.1.0`）を使った deploy フローを確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

1. ターミナル A で `imagod` を起動します。

```bash
cd examples/local-imagod-plugin-hello
cargo run -p imago-cli -- update
cargo run -p imagod -- --config "$(pwd)/imagod.toml"
```

2. ターミナル B で deploy とログ確認を行います。

```bash
cd examples/local-imagod-plugin-hello
cargo run -p imago-cli -- deploy --target default --detach
cargo run -p imago-cli -- logs local-imagod-plugin-hello-app --tail 200
```

## 成功判定

ログに `sizumita:ferris` / `hello from imago` / `called sizumita:ferris/says.say` のいずれかが含まれれば成功です。
