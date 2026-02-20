# local-imagod-http example

## 目的

同一マシンで `type=http` アプリを deploy し、HTTP 応答を確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

```bash
cd examples/local-imagod-http
./scripts/run-imagod.sh
# 別ターミナル
cd examples/local-imagod-http
./scripts/deploy.sh
./scripts/verify-http.sh
```

## 成功判定

`./scripts/verify-http.sh` が `ok: hello from local-imagod-http` を表示すれば成功です。
