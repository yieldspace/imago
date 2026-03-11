# local-imagod-http example

## 目的

同一マシンで `type=http` アプリを service deploy し、HTTP 応答を確認するサンプルです。

## 前提

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。
あわせて OpenSSH client/server を用意し、`ssh localhost true` が対話なしで成功する状態にしてください。
`imago service deploy` は `ssh://localhost?...` 経由で `imagod proxy-stdio` を呼ぶため、SSH ログインシェルの `PATH` から `imagod` バイナリを実行できる必要があります。

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
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-http-app --tail 200
```

## メモリ計測

runner の実メモリ計測は debug ではなく release で実施してください。

```bash
cd /path/to/imago
./scripts/measure_runner_memory.sh examples/local-imagod-http
```

## 成功判定

`imago-cli service logs` の出力に `local-imagod-http-app started` が含まれていれば成功です。

## Troubleshooting

### SSH localhost 経由で service deploy が失敗する

以下を確認してください。

- `ssh localhost true` がパスフレーズ入力や host key 確認なしで成功する
- SSH ログインシェルで `imagod proxy-stdio --socket /tmp/imagod-local-http.sock` を実行できる
- `imago.toml` の `remote = "ssh://localhost?socket=/tmp/imagod-local-http.sock"` と `imagod.toml` の `control_socket_path` が一致している
