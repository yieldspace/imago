# local-imagod example

## 目的

同一マシンで `imagod` を起動し、`imago deploy` の最小フローを確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

```bash
cd examples/local-imagod
./scripts/quickstart.sh
```

`./scripts/quickstart.sh` は deploy 前に `.imagod-data/runtime/ipc/manager-control.sock` が ready になるまで待機します。待機上限は `IMAGOD_READY_TIMEOUT_SECS`（デフォルト: 30秒）で変更できます。

## 成功判定

`./scripts/quickstart.sh` の最後に `ok: local-imagod-app started log detected` が表示されれば成功です。
