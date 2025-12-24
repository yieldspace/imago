# About WIT Plugin

imago では依存関係としてwitを利用して依存関係用いてプラグインを導入することができます。

## プラグインの種類

プラグインには、

1. imagoビルド時に同梱されているネイティブプラグイン
2. Wasm Componentベースのプラグイン

の二種類があります。
プラグインは、`imago.toml`の`[[dependencies]]`に記述することで`imago dev update`コマンドで自動でwitをダウンロードできます。

## プラグインの記述方法

```toml
[[dependencies]]
name = "yieldspace:imago-experimental"
version = "0.0.1"
# プラグインがどのように提供されるか。builtinの場合はimagoに同梱されており、providedの場合はwasmとして提供される。
# type = "provided" # or "builtin"
# `type=provided`の場合ociベースで行われる。

# OCIベースの場合、配信元のregistry.
# registry = "ghcr.io"
```
