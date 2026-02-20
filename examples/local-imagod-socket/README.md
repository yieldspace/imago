# local-imagod-socket example

## 目的

同一マシンで `type=socket` の UDP echo アプリを deploy し、疎通確認するサンプルです。

## 実行

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

```bash
cd examples/local-imagod-socket
./scripts/run-imagod.sh
# 別ターミナル
cd examples/local-imagod-socket
./scripts/deploy.sh
# さらに別ターミナル
printf "hello-udp\n" | nc -u -w 1 127.0.0.1 5000
```

## 成功判定

`imagod` 側ログに受信/送信が出て、同じ payload が返れば成功です。
