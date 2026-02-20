# local-imagod-plugin-hello example

## 目的

同一マシンで Wasm plugin（`warg://sizumita:ferris@0.1.0`）を使った deploy フローを確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

```bash
cd examples/local-imagod-plugin-hello
cargo run --manifest-path ../../Cargo.toml -p imago-cli -- update
./scripts/run-imagod.sh
# 別ターミナル
cd examples/local-imagod-plugin-hello
./scripts/deploy.sh
./scripts/verify-hello.sh
```

## 成功判定

`./scripts/verify-hello.sh` で `sizumita:ferris` 呼び出しを含むログが確認できれば成功です。
