# local-imagod-plugin-camera example

## 目的

同一マシンで Wasm plugin（`imago:camera@0.3.0`）を使い、`imago:v4l2@0.2.0` 上の USB-backed V4L2 camera を OpenCV 風 `VideoCapture` API で操作するサンプルです。
この plugin は V4L2 wrapper のみを提供し、Wasm guest 内に USB/UVC fallback は持ちません。取得フレームは `RGBA8` です。

## 前提

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。
`imago.toml` の `[resources.v4l2].paths` は、実際に使う Linux の `/dev/video*` device node に合わせてください。
`imago.toml` の `remote = "ssh://localhost?socket=/tmp/imagod-local-plugin-camera.sock"` と `imagod.toml` の `control_socket_path` を一致させ、同じユーザーからその socket に接続できる状態にしてください。

## 実行

1. ターミナル A で Wasm app と Wasm plugin を build し、dependency lock を同期してから `imagod` を起動します。

```bash
cd examples/local-imagod-plugin-camera
cargo build --target wasm32-wasip2 --release -p imago-plugin-imago-camera -p local-imagod-plugin-camera-app
cargo run -p imago-cli -- deps sync
cargo run -p imagod -- --config "$(pwd)/imagod.toml"
```

2. ターミナル B で deploy とログ確認を行います。

```bash
cd examples/local-imagod-plugin-camera
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-plugin-camera-app --tail 200
```

## 成功判定

ログに次の文字列が含まれれば成功です。

- `camera example: selected camera index=`
- `camera example: is_opened=true`
- `camera example: set(FrameWidth) ->`
- `camera read frame`
- `camera retrieve frame`

## Troubleshooting

- `[resources.v4l2]` を削ると、`imago:v4l2` 由来の構造化起動エラーになります。
- `camera example: no cameras discovered` が出る場合は `[resources.v4l2].paths` を実在の `/dev/video*` node に合わせてください。
- `open failed` が出る場合は、plugin artifact を再 build してから `imago deps sync` をやり直してください。
