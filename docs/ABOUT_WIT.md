# About WIT Plugin

imago では依存関係としてwitを利用して依存関係用いてプラグインを導入することができます。

## プラグインの種類

プラグインには、

1. imagoビルド時に同梱されているネイティブプラグイン
2. Wasm Componentベースのプラグイン

の二種類があります。
プラグインは、`imago.toml`の`[[dependencies]]`に記述し、`imago update`コマンドで依存WIT/Componentを`.imago/deps/`へ解決できます。`wit/deps/`はこのキャッシュから再生成され、解決結果は`imago.lock`に固定されます。

## プラグインの記述方法

```toml
[[dependencies]]
name = "yieldspace:imago-experimental"
version = "0.0.1"
kind = "native" # or "wasm"
# 省略時: wit.source = "warg://{name}@{version}", wit.registry = "wa.dev"
# wit = "warg://yieldspace:imago-experimental@0.0.1"
# wit = { source = "warg://yieldspace:imago-experimental@0.0.1", registry = "wa.dev" }
# requires = ["yieldspace:imago-core"]

[dependencies.component]
# kind = "wasm" かつ wit が component ではない場合に指定
source = "warg://yieldspace:imago-experimental-component@0.0.1" # or file://...
# registry = "wa.dev" # 省略時 wa.dev
# sha256 = "..." # 省略時は `imago update` が解決して imago.lock に固定

[capabilities]
privileged = false

[capabilities.deps]
"yieldspace:imago-experimental" = ["*"]
```

- `imago update` は依存WIT/Componentを `.imago/deps/` に保存し、そこから `wit/deps/` を再生成します。`imago.lock (version=1)` には `wit_source` / `wit_registry` / `wit_digest` / `wit_path` を固定します。
- `kind="wasm"` で `dependencies.component` を省略した場合、`wit` source が component なら `imago update` が WIT 抽出と `component_source` / `component_registry` / `component_sha256` 固定を自動で行います。
- `warg://` の direct dependency で WIT 側に version が書かれている場合は、`warg://...@version` と一致している必要があります。
- `warg://` の WIT package が transitive import を持つ場合、依存パッケージも `wit/deps/<package>/package.wit` に展開し、`imago.lock` の `[[wit_packages]]` に `requirement` / `version` / `digest` / `source` / `path` / `via` を固定します。
- `.imago_transitive` は使わず、`imago build` は `[[wit_packages]]` の `digest` (`sha256:<hex>`) と `path/package.wit` を照合します。
- plain `.wit` 形式で foreign import を含む source は `imago update` でエラーにします（WIT package 形式が必要）。
- wasm plugin の component 本体は `imago update` 時点で `.imago/deps/<dependency>/components/<sha256>.wasm` に保存します。
- `imago build` / `imago deploy` は source ではなく `.imago/deps/` を参照し、キャッシュ不足時は `imago update` を要求して失敗します。

## 組み込み native plugin（`imago:admin@0.1.0`）

`imagod` には read-only の組み込み native plugin として `imago:admin@0.1.0` が含まれます。
`kind="native"` と `file://` source を使って依存定義できます。
native plugin 実装は `plugins/*` の crate として分離し、descriptor は WIT から macro 生成します。

```toml
[[dependencies]]
name = "imago:admin"
version = "0.1.0"
kind = "native"
wit = "file://../../plugins/imago-admin/wit"

[capabilities.deps]
"imago:admin" = ["*"]
```

`imago:admin/runtime@0.1.0` は以下の 4 関数を提供します。

- `service-name() -> string`
- `release-hash() -> string`
- `runner-id() -> string`
- `app-type() -> string`（`cli` / `http` / `socket`）

`examples/local-imagod-plugin-native-admin` はこの native plugin を使って
runner メタデータを取得する最小例です。

### built-in + 追加 native plugin を使う専用 daemon 例

`imagod` crate は built-in plugin を維持したまま追加 plugin を登録できます。
方式は静的登録（再ビルド前提）です。

```rust
use std::sync::Arc;

use imagod::{
    NativePluginRegistryBuilder, dispatch_from_env_with_registry, register_builtin_native_plugins,
};
use my_plugin::MyNativePlugin;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let mut builder = NativePluginRegistryBuilder::new();
    register_builtin_native_plugins(&mut builder)?;
    builder
        .register_plugin(Arc::new(MyNativePlugin))
        .map_err(anyhow::Error::new)?;

    dispatch_from_env_with_registry(builder.build()).await
}
```

この daemon は manager モードでも runner モードでも同じバイナリを使うため、
`--runner` で起動された子プロセスにも同じ plugin 構成が適用されます。

### NanoKVM 向け native plugin（`imago:nanokvm@0.1.0`）

`plugins/imago-plugin-nanokvm-plugin` は NanoKVM 向け native plugin です。
`capture` に加えて development API を 5 つの interface へ分割して提供します。

依存定義例:

```toml
[[dependencies]]
name = "imago:nanokvm"
version = "0.1.0"
kind = "native"
wit = "file://../../plugins/imago-plugin-nanokvm-plugin/wit"

[capabilities.deps]
"imago:nanokvm" = ["*"]
```

WIT import 例:

```wit
world plugin-imports {
    import imago:nanokvm/capture@0.1.0;
    import imago:nanokvm/stream-config@0.1.0;
    import imago:nanokvm/device-status@0.1.0;
    import imago:nanokvm/runtime-control@0.1.0;
    import imago:nanokvm/hid-control@0.1.0;
    import imago:nanokvm/io-control@0.1.0;
}
```

`imago:nanokvm` が提供する interface:

- `capture`
- `local(auth)`（`http://127.0.0.1:80`）/ `connect(endpoint, auth)`（`http://host[:port]`）/ `session.capture-jpeg()`
  - `capture-jpeg` は `GET /api/stream/mjpeg` の先頭 JPEG frame を抽出し、`wasi:io/streams.input-stream` で返す。
- `stream-config`
  - `get-settings`, `set-stream-type`, `set-resolution`, `set-fps`, `set-quality`, `get-server-config-yaml`
  - `server.yaml` は read-only（取得 API のみ）。
- `device-status`
  - `usb-mode`, `hdmi-status`, `link-status`, `feature-status`, `led-status` を enum で返す。
  - ステータス値が未知だった場合は `unknown` へ丸めず `result::err` を返す。
- `runtime-control`
  - `watchdog` / `stop-ping` の get/set。
- `hid-control`
  - `send-keyboard`, `send-mouse-relative`, `send-mouse-absolute`, `send-touch`, `paste`, `reset-hid`, `get/set-hid-mode`
  - `hid-and-touchpad` / `hid-and-absolute-mouse` は API として公開するが実装は `unsupported` を返す。
- `io-control`
  - `power-pulse`, `reset-pulse`（既定 800ms）。

`auth` は `none` / `token(string)` / `login({username, password})` の 3 方式です。
`token` と `login` はどちらも `nano-kvm-token` cookie を使って認証します。

`stream-config` / `device-status` / `runtime-control` / `hid-control` / `io-control` は
NanoKVM ローカル実行前提（`/kvmapp`, `/etc/kvm`, `/sys`, `/dev/hidg*`）です。
非 NanoKVM 環境では `unsupported` エラーを返します。

`warg://sizumita:ferris@0.1.0` の wasm plugin を使って
`sizumita:ferris/says.say` を呼び出す実行例は
`examples/local-imagod-plugin-hello` を参照してください。
