# imago-with-componentize-js-hono example

## 目的

`componentize-js` と Hono で作った `type=http` の Wasm Component を `imago service deploy` で起動し、HTTP 応答を確認するサンプルです。

## 前提

- Node.js と `pnpm`
- Rust toolchain
- `imago.toml` と `imagod.toml` の socket path が一致していて、同じユーザーからその local control socket に接続できること

`imago service deploy` は `ssh://localhost?...` かつ user/port 未指定のため、`imagod` の `control_socket_path` に直接接続します。
この example は JavaScript/WASI 起動が遅い環境を考慮して、`imagod.toml` で `runner_ready_timeout_secs = 300` を設定しています。

依存は lockfile に固定しているため、先に次を実行してください。

```bash
cd examples/imago-with-componentize-js-hono
pnpm install --frozen-lockfile
```

## ビルド確認

`imago.toml` の `[build].command` と同じコマンドで Wasm Component を生成できます。

```bash
cd examples/imago-with-componentize-js-hono
pnpm run build
wasm-tools component wit dist/component.wasm
```

`wasm-tools component wit` の出力に `export wasi:http/incoming-handler@0.2.6;` が含まれていれば想定どおりです。

## 実行

```bash
# ターミナル1
cd examples/imago-with-componentize-js-hono
cargo run -p imagod -- --config imagod.toml
```

```bash
# ターミナル2
cd examples/imago-with-componentize-js-hono
# ターミナル1 で imagod が起動したことを確認してから実行
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs imago-with-componentize-js-hono --tail 200
curl http://127.0.0.1:18081/hello
```

## 成功判定

- `imago-cli service logs` に `imago-with-componentize-js-hono started` が含まれる
- `curl http://127.0.0.1:18081/hello` が次の JSON を返す

```json
{"message":"Hello from componentize-js + Hono on imago!"}
```

## Troubleshooting

### `pnpm run build` が失敗する

以下を確認してください。

- `pnpm install --frozen-lockfile` を先に実行している
- `dist/component.wasm` が生成されている
- `wasm-tools component wit dist/component.wasm` で `wasi:http/incoming-handler@0.2.6` を確認できる

### localhost 向け `service deploy` が失敗する

以下を確認してください。

- `imago.toml` の `remote = "ssh://localhost?socket=/tmp/imagod-componentize-js-hono.sock"` と `imagod.toml` の `control_socket_path` が一致している
- `imagod` を起動したユーザーと `imago service deploy` を実行したユーザーが同じか、socket file に接続権限がある
- `runner_ready` timeout が出る場合は、`examples/imago-with-componentize-js-hono/imagod.toml` の `runner_ready_timeout_secs` をさらに延ばしてから `imagod` を再起動している

## 参照

- [Hono: WebAssembly (w/ WASI)](https://hono.dev/docs/getting-started/webassembly-wasi)
- [Bytecode Alliance jco Hono example](https://github.com/bytecodealliance/jco/tree/main/examples/components/http-server-hono)
