# local-imagod example

## 目的

同一マシンで `imagod` を起動し、`imago service deploy` の最小フローを確認するサンプルです。

## 前提

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

## 実行

```bash
# ターミナル1
cd examples/local-imagod
cargo run -p imagod -- --config imagod.toml
```

```bash
# ターミナル2
cd examples/local-imagod
# ターミナル1 で imagod が起動したことを確認してから実行
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-app --tail 200
```

## 成功判定

`imago-cli service logs` の出力に `local-imagod-app started` が含まれていれば成功です。

## Troubleshooting

### known_hosts の古いエントリで service deploy が失敗する

`certificate mismatch` などで `service deploy` が失敗する場合のみ、`~/.imago/known_hosts` から `localhost:4443` / `127.0.0.1:4443` の行を削除して再実行してください。
