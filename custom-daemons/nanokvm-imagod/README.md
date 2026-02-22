# nanokvm-imagod

`nanokvm-imagod` は `imagod` を library として利用し、built-in native plugin（`imago:admin`, `imago:node`）に加えて `imago:nanokvm` を静的登録した custom daemon です。

## 起動

```bash
cargo run -p nanokvm-imagod -- --config "$(pwd)/imagod.toml"
```

`--runner` モードは manager が同一バイナリを子プロセス起動するため、追加した plugin 構成がそのまま runner に伝播します。

## `imago.toml` 依存定義例

```toml
[[dependencies]]
name = "imago:nanokvm"
version = "0.1.0"
kind = "native"
wit = "file://../../plugins/imago-plugin-nanokvm-plugin/wit"

[capabilities.deps]
"imago:nanokvm" = ["*"]
```

## WIT 利用例

```wit
package example:nanokvm-client;

world plugin-imports {
    import imago:nanokvm/capture@0.1.0;
    import imago:nanokvm/stream-config@0.1.0;
    import imago:nanokvm/device-status@0.1.0;
    import imago:nanokvm/runtime-control@0.1.0;
    import imago:nanokvm/hid-control@0.1.0;
    import imago:nanokvm/io-control@0.1.0;
}
```

## 提供 interface

- `local(auth)` は `http://127.0.0.1:80` へ接続します。
- `connect(endpoint, auth)` は `http://host[:port]` を受け付けます。
- `session.capture-jpeg()` は JPEG を `wasi:io/streams.input-stream` で返します。
- `stream-config` は stream 設定取得/更新と `server.yaml` read-only 取得を提供します。
- `device-status` は USB/HDMI/network/LED を enum で返し、未知値は `result::err` になります。
- `runtime-control` は watchdog / stop-ping の切替を提供します。
- `hid-control` は keyboard/mouse/touch/paste と HID mode 切替を提供します。
- `io-control` は power/reset pulse を提供します（duration 未指定時 800ms）。

## 制約と注意

- `stream-config` / `device-status` / `runtime-control` / `hid-control` / `io-control` は NanoKVM ローカル環境専用です。非 NanoKVM 環境では `unsupported` を返します。
- `set-hid-mode`, `power-pulse`, `reset-pulse` は危険操作です。呼び出し側で責任を持って実行してください。
- `hid-mode` の `hid-and-touchpad` / `hid-and-absolute-mouse` は現状 `unsupported` を返します。
