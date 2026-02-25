# `imago.toml` Configuration Reference

## 目的と適用範囲

このドキュメントは、`imago.toml` の各キーを実用的に参照するためのリファレンスです。  
実装契約の正本は [`docs/spec/config.md`](./spec/config.md) で、本ページは「読みやすさ重視」の補助資料です。

## 最小構成サンプル

```toml
name = "example-service"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
```

- `type = "http"` の場合は `[http]` が必須です。
- `type = "socket"` の場合は `[socket]` が必須です。
- `type = "cli"` / `type = "rpc"` では `[http]` / `[socket]` は指定しません。

## セクション一覧

- トップレベルキー (`name`, `main`, `type`, `restart`)
- `[build]`
- `[vars]` (legacy)
- `[secrets]` (legacy)
- `[target.<name>]`
- `[http]`
- `[socket]`
- `[[assets]]`
- `[wasi]`
- `[capabilities]`
- `[namespace_registries]`

## キーごとのリファレンス

### トップレベルキー

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `name` | 必須 | string | 1..=63 文字、ASCII `[A-Za-z0-9._-]`、`/` `\\` `..` 禁止 | なし | `name = "example-service"` |
| `main` | 必須 | string (relative path) | 非空、相対パスのみ、backslash/drive prefix/path traversal 禁止、実ファイル存在必須 | なし | `main = "build/app.wasm"` |
| `type` | 必須 | string enum | `cli` / `http` / `socket` / `rpc` のいずれか | なし | `type = "http"` |
| `restart` | 任意 | string enum | `never` / `on-failure` / `always` / `unless-stopped` | `never` | `restart = "always"` |

### `[build]`

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `build.command` | 任意 | string OR array(string) | string は非空、array は非空かつ全要素 string | なし | `command = "cargo component build --release"` |

### `[vars]` (legacy)

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `vars.<KEY>` | 任意 | string | table の値は string のみ。manifest には出力されない | 空 table | `APP_MODE = "prod"` |

### `[secrets]` (legacy)

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `secrets.<KEY>` | 任意 | string | table の値は string のみ。manifest には出力されない | 空 table | `SECRET_TOKEN = "change-me"` |

### `[target.<name>]`

`<name>` は target 名です。CLI では通常 `default` target が使われます。

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `target.<name>.remote` | 選択 target では必須 | string | endpoint 文字列。deploy/run 経路で形式検証 | なし | `remote = "127.0.0.1:4443"` |
| `target.<name>.server_name` | 任意 | string | string であること | なし | `server_name = "node-a.example.com"` |
| `target.<name>.client_key` | build では任意、deploy では必須 | string (path) | 非空、backslash/drive prefix/path traversal 禁止、相対は project root 基準、絶対パス可 | なし | `client_key = "certs/client.key"` |

### `[http]` (`type = "http"` のとき必須)

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `http.port` | `type = "http"` で必須 | integer | `1..=65535` | なし | `port = 8080` |
| `http.max_body_bytes` | 任意 | integer | `1..=67108864` (64 MiB) | `8388608` (8 MiB) | `max_body_bytes = 8388608` |

### `[socket]` (`type = "socket"` のとき必須)

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `socket.protocol` | `type = "socket"` で必須 | string enum | `udp` / `tcp` / `both` | なし | `protocol = "tcp"` |
| `socket.direction` | `type = "socket"` で必須 | string enum | `inbound` / `outbound` / `both` | なし | `direction = "inbound"` |
| `socket.listen_addr` | `type = "socket"` で必須 | string | 有効な IP アドレス literal | なし | `listen_addr = "0.0.0.0"` |
| `socket.listen_port` | `type = "socket"` で必須 | integer | `1..=65535` | なし | `listen_port = 9000` |

### `[[assets]]`

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `assets[].path` | 各 entry で必須 | string (relative path) | 非空、`main` と同等の相対パス制約、実ファイル存在必須 | なし | `path = "assets/config.json"` |

- `[[assets]]` の追加キーは受理され、manifest に JSON 値として転送されます。

### `[wasi]`

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `wasi.args` | 任意 | array(string) | 各要素は非空 string | 空 array | `args = ["--serve"]` |
| `wasi.http_outbound` | 任意 | array(string) | `hostname` / `host:port` / `CIDR`。wildcard 不可。CIDR は request host が IP literal の場合のみ評価 | 空 array (manager 側で localhost 系を注入) | `http_outbound = ["localhost", "api.example.com:443", "10.0.0.0/8"]` |
| `wasi.env.<KEY>` | 任意 | string | key/value とも string。`project_root/.env` が同名キーを上書き | 空 table | `WASI_ONLY = "1"` |
| `wasi.mounts[].asset_dir` | 任意 | string | `assets[].path` 親ディレクトリ由来のみ指定可能。`read_only_mounts` と重複不可 | なし | `asset_dir = "assets"` |
| `wasi.mounts[].guest_path` | 任意 | string | 絶対 unix path、backslash/path traversal 禁止。`read_only_mounts` と重複不可 | なし | `guest_path = "/app/assets"` |
| `wasi.read_only_mounts[].asset_dir` | 任意 | string | `assets[].path` 親ディレクトリ由来のみ指定可能。`mounts` と重複不可 | なし | `asset_dir = "readonly"` |
| `wasi.read_only_mounts[].guest_path` | 任意 | string | 絶対 unix path、backslash/path traversal 禁止。`mounts` と重複不可 | なし | `guest_path = "/app/readonly"` |

### `[capabilities]`

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `capabilities.privileged` | 任意 | boolean | bool のみ | `false` | `privileged = false` |
| `capabilities.deps` | 任意 | string `"*"` OR table(map<string, array(string)>) | string は `"*"` のみ。table は dependency package 名または `"*"` を key にとり、値は関数名配列 | 空 table | `deps = "*"` |
| `capabilities.wasi` | 任意 | boolean OR table(map<string, array(string)>) | `true` は wildcard allow、`false` は空 policy。table は interface ごとの関数許可 | 空 table | `wasi = true` |

### `[namespace_registries]`

| キー | 必須 | 型 | 制約 | 既定値 | 例 |
|---|---|---|---|---|---|
| `namespace_registries.<namespace>` | 任意 | string | registry host 文字列。`warg://` source の registry 省略時のみ適用 | なし | `"wasi" = "wasi.dev"` |

## 型別・用途別の注意点

### `type = "cli"`

- `[http]` / `[socket]` は指定しません。
- `main` の実行に必要な権限は `[capabilities]` と `[wasi]` で明示します。

### `type = "http"`

- `[http].port` は必須です。
- body 制限を明示したい場合は `[http].max_body_bytes` を指定します。

### `type = "socket"`

- `[socket]` 4キー (`protocol`, `direction`, `listen_addr`, `listen_port`) が必須です。
- `listen_addr` は hostname ではなく IP literal を使います。

### `type = "rpc"`

- `[http]` / `[socket]` は不要です。
- service 間通信は `[[bindings]]` と `[capabilities.deps]` を併用して制御します。

## エラーになりやすい設定例

### 1. `type` とセクション不整合

```toml
type = "http"

[socket]
protocol = "tcp"
direction = "inbound"
listen_addr = "0.0.0.0"
listen_port = 9000
```

`type = "http"` で `[socket]` を指定すると検証エラーになります。

### 2. `restart` の不正値

```toml
restart = "sometimes"
```

`restart` は `never` / `on-failure` / `always` / `unless-stopped` のみ許可されます。

### 3. `wasi.http_outbound` の wildcard 指定

```toml
[wasi]
http_outbound = ["*.example.com"]
```

wildcard は未対応のためエラーになります。

### 4. mount 重複

```toml
[[wasi.mounts]]
asset_dir = "assets"
guest_path = "/app/data"

[[wasi.read_only_mounts]]
asset_dir = "assets"
guest_path = "/app/data"
```

`asset_dir` / `guest_path` の重複は配列間でも禁止です。

## 関連仕様リンク

- 正本: [`docs/spec/config.md`](./spec/config.md)
- manifest 反映: [`docs/spec/manifest.md`](./spec/manifest.md)
- `imagod` 概要: [`docs/spec/imagod.md`](./spec/imagod.md)
