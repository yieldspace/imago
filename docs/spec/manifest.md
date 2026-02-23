# Manifest Specification (`build/manifest.json`)

## 目的

`build/manifest.json` の固定フォーマットを定義し、CLI と runtime の入力仕様を一致させる。

関連仕様:

- 設定入力の正規化: [`config.md`](./config.md)
- 転送・実行仕様: [`deploy-protocol.md`](./deploy-protocol.md)

## 出力場所

- 固定パス: `build/manifest.json`

<a id="required-fields"></a>
## 必須フィールド

| フィールド | 型 | 説明 |
|---|---|---|
| `name` | string | サービス名 |
| `main` | string | Wasm エントリパス |
| `type` | string | `cli` / `http` / `socket` / `rpc` |
| `target` | object | 解決済みターゲット設定 |
| `assets` | array | 同梱アセット一覧 |
| `bindings` | array | service 間呼び出し許可一覧（省略時は `[]`） |
| `http` | object | `type=http` 時の HTTP 実行設定（`port` 必須） |
| `socket` | object | `type=socket` 時の socket 実行設定（必須） |
| `wasi` | object | WASI 実行設定（`args` / `env` / `mounts` / `read_only_mounts`） |
| `dependencies` | array | typed plugin 依存解決結果 |
| `capabilities` | object | 正規化済み capability ルール（省略時は deny-by-default） |
| `hash` | object | 全体整合性情報 |

## `hash` フィールド

`hash` は以下を必須とする。

- `algorithm`: `sha256`
- `value`: 計算済み digest
- `targets`: `wasm`, `manifest`, `assets`

<a id="hash-targets"></a>
## ハッシュ対象ルール

全体 hash は次を対象に計算する。

1. `manifest.main` が指す wasm のバイト列
2. `manifest.json` の正規化 JSON バイト列
3. `assets` 配下ファイルのバイト列（パス昇順）

`hash.targets` に `wasm` / `manifest` / `assets` が揃っていない場合は不正 manifest とみなす。

<a id="wasi-env-bundling"></a>
## `wasi.env` 同梱方針

- 環境変数は `manifest.wasi.env` に同梱してデプロイ時に送信する。
- runtime 側は `wasi.env` の実値をログへ出力してはいけない。
- CLI 側は `--dry-run` を除き `wasi.env` の実値を表示してはいけない。

## `bindings` フィールド

- `bindings` は `[{"name": "<service>", "wit": "<package>/<interface>"}, ...]` の配列。
- `name` は service 名文字制約（`name` と同等）に従う。
- `wit` は `<package>/<interface>` 形式の非空文字列。
- `imago update` は `imago.toml` の `[[bindings]].wit` から解決した WIT package 内の全 interface を展開し、この形式で `bindings` を出力する。
- `bindings` 未指定 manifest は `[]` と同等に扱う（後方互換）。

## `dependencies` フィールド

- `dependencies` は typed 構造で出力する。
  - `name`, `version`, `kind`, `wit`, `requires`, `component`, `capabilities`
- `kind` は `native` / `wasm`。
- `kind=wasm` の場合、`component.path` / `component.sha256` を出力する。
  - `imago.toml` 側で `component` 未指定でも、`wit` source が component なら `imago update` が lock に `component_*` を固定し、`imago build` が manifest の `component.*` を生成する。
  - `component.path` は `imago build` が `plugins/components/<sha256>.wasm` へ正規化して出力する。
  - `component.sha256` は `imago.lock.dependencies[].component_sha256` と一致する。
- `capabilities` は plugin caller 用のルールで、`privileged` / `deps` / `wasi` を受理する。
- runtime の transitive import 解決順は `self(component export)` -> `dependencies 内の package名一致` -> `error`。
- `requires` は順序ヒントとして保持するが、transitive import 解決の必須条件ではない。

## `capabilities` フィールド

- ルート `manifest.capabilities` は app caller 用のルール。
- `privileged=true` の場合は全許可。
- それ以外は `deps` / `wasi` で明示許可された関数のみ許可（default deny）。
- `deps` は dependency package 名（または wildcard `*`）をキーに、許可関数配列を定義する。
  - wildcard key `deps."*"` は任意 dependency へ適用する。
  - 同一 dependency に明示キーと wildcard が両方ある場合、明示キーを優先する。
- `capabilities.wasi` は `true` / `false` または table を受理する。
  - `true`: 全 WASI を許可。
  - `false`: 空ルールとして扱い、WASI を許可しない。
  - table: `capabilities.wasi.<interface>` ごとに `"*"` または関数名文字列配列で許可関数を定義する。
- self 解決（caller 自身の component export）には `deps` 認可を要求しない。

## `http` フィールド

- `http` は `type=http` のときのみ許可する。
- `http.port` は必須で `1..=65535`。
- `http.max_body_bytes` は必須で `1..=67108864`（64MiB）。
- 旧 manifest 互換のため `http.max_body_bytes` 欠落時は runtime 側で `8388608`（8MiB）として解釈できること。
- `type!=http` で `http` を含む manifest は不正として拒否する。

## `socket` フィールド

- `socket` は `type=socket` のとき必須。
- `socket.protocol` は `udp` / `tcp` / `both`。
- `socket.direction` は `inbound` / `outbound` / `both`。
- `socket.listen_addr` は IP アドレス文字列（IPv4/IPv6）。
- `socket.listen_port` は必須で `1..=65535`。
- `type!=socket` で `socket` を含む manifest は不正として拒否する。

## `wasi` フィールド

- `wasi` は任意の object。
- `wasi.args` は string 配列。
- `wasi.env` は `key -> value` がともに string の table。
- `wasi.mounts` は read/write mount 配列。
- `wasi.read_only_mounts` は read-only mount 配列。
- `wasi.mounts[]` / `wasi.read_only_mounts[]` の各要素は `asset_dir` と `guest_path` を必須とする。
  - `asset_dir` は `assets` 由来ディレクトリのみ受理する（mount 単位はディレクトリのみで、ファイル単位は不可）。
  - `guest_path` は guest 側の絶対パスのみ受理する。
- `wasi.mounts[]` と `wasi.read_only_mounts[]` では、同一 `guest_path` または同一 `asset_dir` の重複指定（同一配列内および配列跨ぎ）を禁止する。
- runtime 権限マッピングは次を適用する。
  - `wasi.mounts[]`: `DirPerms::all` / `FilePerms::all`
  - `wasi.read_only_mounts[]`: `DirPerms::READ` / `FilePerms::READ`

## 正常例と異常例

- 正常例: [`examples/manifest.valid.json`](./examples/manifest.valid.json)
- 異常例（必須欠落）: [`examples/manifest.invalid.missing-required.json`](./examples/manifest.invalid.missing-required.json)
- 異常例（型不正）: [`examples/manifest.invalid.bad-type.json`](./examples/manifest.invalid.bad-type.json)
- 異常例（hash 検証不一致）: [`examples/manifest.invalid.hash-mismatch.json`](./examples/manifest.invalid.hash-mismatch.json)
- 異常例（`wasi.env` 形式不正）: [`examples/manifest.invalid.wasi-env-shape.json`](./examples/manifest.invalid.wasi-env-shape.json)

## バリデーション要件

- 必須フィールド欠落は拒否。
- `type` が定義外なら拒否。
- `type=http` かつ `http.port` 欠落は拒否。
- `type=http` かつ `http.max_body_bytes` が範囲外（`1..=67108864`）は拒否。
- `type!=http` かつ `http` 指定は拒否。
- `type=socket` かつ `socket` 欠落は拒否。
- `type=socket` かつ `socket.protocol` / `socket.direction` が定義外値なら拒否。
- `type=socket` かつ `socket.listen_addr` が IP として不正なら拒否。
- `type=socket` かつ `socket.listen_port` が範囲外（`1..=65535`）なら拒否。
- `type!=socket` かつ `socket` 指定は拒否。
- `hash.algorithm != "sha256"` は拒否。
- `hash.targets` が不足または重複なら拒否。
- `bindings` 指定時は配列のみ許可し、各要素は `name` / `wit` の非空文字列を必須とする。
- `bindings[].wit` は `<package>/<interface>` 形式のみ許可する。
- `dependencies` 指定時は typed 構造のみ許可し、`kind=wasm` は `component.path` / `component.sha256` を必須とする（`imago build` 生成物として）。
- `capabilities` は `privileged` / `deps` / `wasi` 以外のキーを拒否する。
- `capabilities.wasi` は `bool` または table 以外を拒否する。
- `wasi.args` は string 配列以外を拒否する。
- `wasi.env` は string 値の table 以外を拒否する。
- `wasi.mounts[]` / `wasi.read_only_mounts[]` の要素に `asset_dir` または `guest_path` 欠落がある場合は拒否する。
- `wasi.mounts[]` / `wasi.read_only_mounts[]` の `asset_dir` が `assets` 由来ディレクトリ以外、またはファイル単位の場合は拒否する。
- `wasi.mounts[]` / `wasi.read_only_mounts[]` の `guest_path` が絶対パスでない場合は拒否する。
- `wasi.mounts[]` / `wasi.read_only_mounts[]` で同一 `guest_path` または同一 `asset_dir` が重複（同一配列内・配列跨ぎ）した場合は拒否する。

## 実装ノート

- manifest は deploy リクエストの入力正本とする。
- runtime 側での再計算結果が `hash.value` と一致しない場合は整合性エラーとして扱う。

## 実装反映ノート

- CLI の `hash.value` 計算は `hash.value` を空文字にした中間 manifest JSON を使って実行する。
  - 連結順序は `main`（wasm bytes）→ 中間 manifest JSON bytes → assets bytes（`path` 昇順）。
- CLI は `main` の実体 wasm を `build/<sha256>-<name>.wasm` へ配置し、`manifest.main` には manifest ファイル同階層基準の相対パス（`<sha256>-<name>.wasm`）を書き込む。
- `build/<sha256>-<name>.wasm` が既に存在する場合でも、内容の sha256 が不一致なら `main` の実体 wasm から上書き再生成する。
- `hash.value` の wasm 対象は `manifest.main` が指す materialize 後ファイルとする。
- CLI は `imago.toml` の `[[bindings]].wit` を `imago update` で解決し、package 内の全 interface を `manifest.bindings[]` の `<package>/<interface>` として正規化して出力する。
- [BREAKING] `imago.toml` の `[[bindings]].wit` で旧 `"<package>/<interface>"` 形式は受理しない。
- CLI は `imago.toml` の `[[dependencies]]` を typed `manifest.dependencies[]` に正規化し、lock 検証済みの WIT/Component 参照情報を保持する。
  - `kind=wasm` で `component` 未指定の場合、`wit` source が component なら `imago update` が `component_*` を lock に自動固定し、`imago build` が manifest の `component.*` を補完する。
- CLI は `imago.toml` の `capabilities` を正規化して `manifest.capabilities` に出力する（`capabilirties` は互換受理しない）。
- CLI は `capabilities.deps` に `string("*")` / table の両形式を受理し、`"*"` は `{"*": ["*"]}` として正規化する。
- CLI は `capabilities.wasi` に `bool` / table の両形式を受理し、`true` は全許可、`false` は空ルールとして正規化する。
- CLI は `type=http` 時のみ `imago.toml` の `[http].port` / `[http].max_body_bytes` を `manifest.http.port` / `manifest.http.max_body_bytes` へ正規化して出力する。
- CLI は `type=socket` 時のみ `imago.toml` の `[socket].protocol` / `[socket].direction` / `[socket].listen_addr` / `[socket].listen_port` を `manifest.socket.*` へ正規化して出力する。
- CLI は `[wasi]` を `manifest.wasi`（`args` / `env` / `mounts` / `read_only_mounts`）へ正規化して出力し、`project_root/.env` のキーを `manifest.wasi.env` へ追加・上書きする（`.env` 優先）。
- CLI は `imago.toml` の legacy `[vars]` / `[secrets]` を受理するが、manifest には出力しない。
- runtime は `wasi.mounts[]` を `DirPerms::all` / `FilePerms::all`、`wasi.read_only_mounts[]` を `DirPerms::READ` / `FilePerms::READ` として適用する。

## 実装反映ノート（Network RPC / 2026-02-18）

- [BREAKING] `type` に `rpc` を追加した。
- [BREAKING] `bindings` の契約を `target` から `name` へ変更した。
