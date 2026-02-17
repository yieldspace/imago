# About WIT Plugin

imago では依存関係としてwitを利用して依存関係用いてプラグインを導入することができます。

## プラグインの種類

プラグインには、

1. imagoビルド時に同梱されているネイティブプラグイン
2. Wasm Componentベースのプラグイン

の二種類があります。
プラグインは、`imago.toml`の`[[dependencies]]`に記述し、`imago update`コマンドでWITを`wit/deps/`へ解決できます。解決結果は`imago.lock`に固定されます。

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
# kind = "wasm" のとき必須
source = "warg://yieldspace:imago-experimental-component@0.0.1" # or file://...
# registry = "wa.dev" # 省略時 wa.dev
# sha256 = "..." # 省略時は `imago update` が解決して imago.lock に固定

[capabilities]
privileged = false

[capabilities.deps]
"yieldspace:imago-experimental" = ["*"]
```

- `imago update` は WIT を `wit/deps/` に展開し、`imago.lock` に `wit_source` / `wit_registry` / `wit_digest` / `wit_path` を固定します。
- `warg://` の direct dependency で WIT 側に version が書かれている場合は、`warg://...@version` と一致している必要があります。
- `warg://` の WIT package が transitive import を持つ場合、依存パッケージも `wit/deps/<package>/package.wit` に展開します。
- plain `.wit` 形式で foreign import を含む source は `imago update` でエラーにします（WIT package 形式が必要）。
- wasm plugin の component 本体は `imago update` では取得せず、`component_sha256` のみ lock に固定します。
- `imago deploy` は lock の `component_source` / `component_registry` / `component_sha256` を使って component を取得し、`.imago/components/<sha256>.wasm` を再利用します。

`warg://chikoski:hello-world@0.2.0` の wasm plugin を使って
`chikoski:hello/greet.hello` を呼び出す実行例は
`examples/local-imagod-plugin-hello` を参照してください。
