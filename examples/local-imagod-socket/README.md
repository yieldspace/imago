# local-imagod-socket example

## 目的

同一マシンで `type=socket` の UDP echo アプリを service deploy し、疎通確認するサンプルです。

## 前提

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。
`imago.toml` の `remote = "ssh://localhost?socket=/tmp/imagod-local-socket.sock"` と `imagod.toml` の `control_socket_path` を一致させ、同じユーザーからその socket に接続できる状態にしてください。

## 実行

```bash
# ターミナル1
cd examples/local-imagod-socket
cargo run -p imagod -- --config imagod.toml
```

```bash
# ターミナル2
cd examples/local-imagod-socket
# ターミナル1 で imagod が起動したことを確認してから実行
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-socket-app --tail 200
```

## 成功判定

`imago-cli service logs` の出力に `local-imagod-socket-app listening on udp://0.0.0.0:5000` が含まれていれば成功です。

## Troubleshooting

### localhost 向け service deploy が失敗する

以下を確認してください。

- `imago.toml` の `remote = "ssh://localhost?socket=/tmp/imagod-local-socket.sock"` と `imagod.toml` の `control_socket_path` が一致している
- `imagod` を起動したユーザーと `imago service deploy` を実行したユーザーが同じか、socket file に接続権限がある
