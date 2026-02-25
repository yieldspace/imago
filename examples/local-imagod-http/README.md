# local-imagod-http example

## 目的

同一マシンで `type=http` アプリを deploy し、HTTP 応答を確認するサンプルです。

## 前提

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

## 実行

```bash
# ターミナル1
cd examples/local-imagod-http
cargo run -p imagod -- --config imagod.toml
```

```bash
# ターミナル2
cd examples/local-imagod-http
# ターミナル1 で imagod が起動したことを確認してから実行
cargo run -p imago-cli -- deploy --target default --detach
cargo run -p imago-cli -- logs local-imagod-http-app --tail 200
```

## 成功判定

`imago-cli logs` の出力に `local-imagod-http-app started` が含まれていれば成功です。

## Troubleshooting

### known_hosts の古いエントリで deploy が失敗する

`certificate mismatch` などで `deploy` が失敗する場合のみ、`~/.imago/known_hosts` から `localhost:4443` / `127.0.0.1:4443` の行を削除して再実行してください。
