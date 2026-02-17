# Configuration Specification (`imago.toml`)

## 目的

`imago.toml` の必須項目、上書き規則、権限モデル、既定値、検証条件を固定し、CLI と runtime が同じ解釈で動くようにする。

関連仕様:

- マニフェストへの反映規則: [`manifest.md`](./manifest.md)
- デプロイ時の利用方法: [`deploy-protocol.md`](./deploy-protocol.md)

## 用語

- base 設定: `imago.toml` のトップレベル設定。
- env 設定: `[env.<name>]` 配下の上書き設定。
- capabilities: runtime で明示許可する権限。

<a id="required-keys"></a>
## 必須キー

| キー | 型 | 制約 | 説明 |
|---|---|---|---|
| `name` | string | 1-63 文字、空文字不可、`/` `\` `..` 禁止 | サービス識別名 |
| `main` | string | 相対パス、空文字不可 | 実行対象の Wasm パス |
| `type` | string | `cli` / `http` / `socket` のいずれか | 実行モデル |
| `target` | table | 必須 | デプロイ先設定 |

`name` の許可文字は ASCII 英数字、`.`、`-`、`_`。`/`、`\`、`..` は path 文字として拒否する。

## 推奨キー

- `args`
- `build`
- `capabilities`
- `limits`
- `restart`
- `vars`
- `assets`
- `dependencies`
- `bindings`

<a id="env-override"></a>
## `--env` 上書き規則

1. `--env` 未指定時は base 設定のみを使う。
2. `--env <name>` 指定時は `[env.<name>]` を base 設定にマージする。
3. `--env <name>` 指定時に読み込む環境変数ファイルは `.env.<name>` のみ。
4. マージ範囲はトップレベルキー単位。`[env.<name>]` で指定したキーは base 側の同名キーを丸ごと置換する。
5. 指定された env 名が存在しない場合はエラー。

`<name>` の許可文字は ASCII 英数字、`.`、`-`、`_`。`/`、`\`、`..` は禁止する。

## build コマンド設定

- `imago build` は `imago.toml` の `[build].command` を参照する。
- `build.command` は次のいずれかを受け付ける。
  - string: `sh -c "<command>"` として実行
  - array: `["cmd", "arg1", ...]` として直接実行
- `build.command` 未指定時はビルドコマンドを実行せず、`main` の存在検証のみ行う。
- `--env <name>` 指定時は `.env.<name>` の値を build サブプロセス環境へ注入する。

## `[[bindings]]`（service 間呼び出し許可）

- `[[bindings]]` は service 間関数呼び出しの許可ルールを定義する。
- 各要素は以下を必須とする。
  - `target`: 呼び出し先 service 名（`name` と同じ文字制約）
  - `wit`: interface 識別子文字列
- `imago build` はこの設定を `manifest.bindings[]` に正規化して出力する。
- 未指定時は `manifest.bindings=[]` として扱い、runtime は deny-by-default で拒否する。

## `[[dependencies]]`（プラグイン依存）

- `[[dependencies]]` は plugin 依存を定義する。
- 各要素は以下を受理する。
  - `name` (必須): package 名
    - 許可文字は ASCII 英数字、`.`、`_`、`-`、`:`, `/`。
    - `/` は package 階層表現として許可するが、path component は `Normal` のみ許可する（先頭 `/`、drive prefix、`./`、`../` などは拒否）。
  - `version` (必須): version 文字列
  - `kind` (必須): `native` / `wasm`
  - `wit` (任意): string または table
    - string は `file://...` / `warg://...` を受理
    - table は `wit.source`（必須）+ `wit.registry`（任意）を受理
    - 未指定時は `wit.source = "warg://{name}@{version}"` / `wit.registry = "wa.dev"`
  - `requires` (任意): 依存 plugin package 名配列（明示依存の宣言）
    - 各要素のバリデーションは `name` と同一。
  - `component.source` (任意, `kind=wasm` の場合): `file://...` / `warg://...`
    - `wit` source が component ではない場合は指定が必要
  - `component.registry` (任意, `kind=wasm` の場合): `warg://` の registry（省略時 `wa.dev`）
  - `component.sha256` (任意, `kind=wasm` の場合): 指定時は `imago update` で照合
  - `capabilities` (任意): この plugin が caller になる場合の認可ルール
- `imago update` は依存の WIT を `wit/deps/` に展開し、`imago.lock (version=1)` に `wit_source` / `wit_registry` / `wit_digest` / `wit_path` / `resolved_at` を固定する。`wit_path` の依存名サニタイズは wkg 準拠で `:` / `@` を `-` に置換する。
- `warg://` の direct dependency で WIT 側に version が書かれている場合は、`warg://...@version` と一致している必要がある。
- `warg://` の WIT package が transitive import を含む場合、依存パッケージも `wit/deps/<package>/package.wit` に展開し、`imago.lock.[[wit_packages]]` に `name` / `registry` / `[[versions]]` (`requirement` / `version` / `digest` / `source` / `path` / `via`) を固定する。
- `dependencies[].wit.source` が `file://...` の場合、`wit/deps` 配下を指す source は禁止する（`imago update` が `wit/deps` を再生成するため）。
- 複数 dependency が同一 `wit/deps` 出力先へ解決される場合、`imago update` は衝突として失敗する。
- `warg://` source が plain `.wit` 形式で foreign import を含む場合は `imago update` を失敗させる（WIT package 形式が必要）。
- `kind=wasm` かつ `component` 未指定で `wit` source が component の場合、`imago update` は component から WIT を抽出し、lock の `component_*` を自動固定する。
- `kind=wasm` かつ `component` 未指定で `wit` source が component に解釈できない場合、`imago update` は失敗する。
- `kind=wasm` では `imago.lock` に `component_source` / `component_registry` / `component_sha256` も固定する（component本体はこの時点で保存しない）。
- `imago build` は `imago.lock` が未生成、`version != 1`、または lock digest 不一致のとき失敗し、`imago update` を要求する。
- `.imago_transitive` は使用しない。transitive package 検証は `imago.lock.[[wit_packages]]` の `digest` と `path/package.wit` の照合で行う。
- `imago deploy` は `imago.lock` の component 情報を使って必要時に component を取得し、`.imago/components/<sha256>.wasm` を再利用する。
- runtime の transitive plugin import 解決順は `self(component export)` -> `明示 dependency(package名一致)` -> `error`。
- transitive 解決では `requires` の記述を必須にしない。

## `[http]`（`type=http` 時の ingress 設定）

- `type = "http"` の場合のみ `[http]` セクションを受理する。
- `http.port` は必須で、`1..=65535` の整数のみ許可する。
- `http.max_body_bytes` は任意で、`1..=67108864`（64MiB）の整数のみ許可する。
- `http.max_body_bytes` 未指定時は `8388608`（8MiB）を既定値として扱う。
- `type != "http"` で `[http]` を指定した場合は設定不整合として build エラーにする。
- `imago build` はこの設定を `manifest.http.port` / `manifest.http.max_body_bytes` に正規化して出力する。

## `[socket]`（`type=socket` 時の socket 実行設定）

- `type = "socket"` の場合のみ `[socket]` セクションを受理する。
- `socket.protocol` は必須で `udp` / `tcp` / `both` のいずれか。
- `socket.direction` は必須で `inbound` / `outbound` / `both` のいずれか。
- `socket.listen_addr` は必須で IP アドレス文字列（IPv4/IPv6）を受理する。
- `socket.listen_port` は必須で `1..=65535` の整数のみ許可する。
- `type != "socket"` で `[socket]` を指定した場合は設定不整合として build エラーにする。
- `imago build` はこの設定を `manifest.socket.*` に正規化して出力する。

## `restart`（service 再起動方針）

- `restart` はトップレベルキーとして指定する。
- 許可値は `never` / `on-failure` / `always` / `unless-stopped`。
- 未指定時の既定値は `never`。
- `imagod` 起動時の自動復元対象は `restart="always"` の service のみ。
- `on-failure` / `unless-stopped` の高度な再起動戦略は現行未実装で、値の受理・保存のみ行う。
- 旧キー `runtime.restart_policy` は受理しない（設定エラー）。

<a id="capability-model"></a>
## 権限モデル

### 既定挙動

- `capabilities` 未指定時は全拒否（deny-by-default）。

### `capabilities`

- `capabilities.deps.<package>`: 依存 plugin の呼び出し許可関数。
  - 許可値は `"*"` または関数名文字列配列。
  - self 解決（caller 自身の component export）には適用しない。
  - 明示 dependency への中継時のみ適用する。
- `capabilities.wasi.<interface>`: WASI interface ごとの許可関数。
  - 許可値は `"*"` または関数名文字列配列。
- typo キー `capabilirties` は互換受理せずエラーにする。

### `privileged`

- `privileged = true` の場合、`capabilities` は無視し全許可。
- `privileged` 未指定時は `false` として扱う。

<a id="defaults"></a>
## 既定値

| キー | 既定値 | 備考 |
|---|---|---|
| `limits.shutdown_timeout` | `30s` | graceful 停止待ち時間 |
| `restart` | `never` | `imago.toml` のトップレベルキー |

## バリデーション要件

- 必須キー欠落はエラー。
- `type` 不正値はエラー。
- `type="http"` かつ `http.port` 欠落はエラー。
- `type="http"` かつ `http.max_body_bytes` が範囲外（`1..=67108864`）はエラー。
- `type!="http"` かつ `[http]` 指定はエラー。
- `type="socket"` かつ `[socket]` 欠落はエラー。
- `type="socket"` かつ `socket.protocol` / `socket.direction` が定義外値ならエラー。
- `type="socket"` かつ `socket.listen_addr` が IP として不正ならエラー。
- `type="socket"` かつ `socket.listen_port` が範囲外（`1..=65535`）ならエラー。
- `type!="socket"` かつ `[socket]` 指定はエラー。
- `restart` が許可値（`never` / `on-failure` / `always` / `unless-stopped`）以外ならエラー。
- `runtime.restart_policy` を指定した場合はエラー（互換受理なし）。
- `dependencies[].wit` に `https://wa.dev/...` shorthand を指定した場合はエラー（`warg://<package>@<version>` を使用）。
- `dependencies[].wit.source` に `file://wit/deps/...`（または同等の `wit/deps` 配下パス）を指定した場合はエラー。
- 複数 dependency が同一 `wit/deps` 出力パスへ解決される場合はエラー。
- `dependencies[].name` と `dependencies[].requires[]` に、絶対パス・drive prefix・`./`・`../` を含む path component を指定した場合はエラー。
- `[[dependencies]]` 使用時に `imago.lock` が存在しない、`version != 1`、または lock の `wit_*` / `component_*` / `wit_packages` が設定・展開結果と一致しない場合はエラー。
- `main` が存在しない場合はビルド時エラー。
- `shutdown_timeout` が 0 以下はエラー。
- `privileged = true` かつ `capabilities` 指定ありでもエラーにはしない（`capabilities` を無視）。

## 異常系

- 存在しない env 指定。
- 不正な env 名（`/`、`\`、`..` を含む、または許可文字外を含む）。
- `.env.<name>` の読み込み失敗。
- 型不正（例: `shutdown_timeout = "abc"`）。
- 不正な `type`。

## 実装ノート

- 設定ロードは CLI 側で厳格検証し、正規化結果を [`manifest.md`](./manifest.md) の形式で出力する。
- runtime 側は manifest を信頼入力として扱い、再解釈を最小化する。

## 実装反映ノート

- `[env.<name>]` の反映はトップレベルキー単位の置換で実装する。
- `build.command` は string / array の両形式を受理する。
- `build.command` は必須キー (`name`/`main`/`type`/`target`) と `vars`/`dependencies` の検証完了後に実行する。不正設定時は実行しない。
- `imago update` は `warg://` / `file://` を受理し、WIT を `wit/deps/` へ展開する。
- `imago update` の dependency path サニタイズは wkg 準拠 (`:` / `@` を `-`) を使い、`wit/deps` と `.imago/warg` の両方で同じ命名規則を使う。
- `imago update` は `dependencies[].wit.source=file://...` が `wit/deps` 配下を指す場合と、dependency 同士で `wit/deps` 出力先が衝突する場合に、`wit/deps` を削除する前に失敗させる。
- dependency package 名の path バリデーションは `/` を許可しつつ、`Path` component が `Normal` 以外（`RootDir`、`Prefix`、`CurDir`、`ParentDir`）を拒否する。
- `warg://` は Rust-native client で解決し、`registry` 未指定時は `wa.dev` を使う。
- `imago update` は `kind=wasm` かつ `component` 未指定で `wit` source が component の場合、component hash/source を lock へ自動固定する。
- `imago build` は `capabilities` を正規化して manifest に出力し、`capabilirties` キーは設定エラーとして拒否する。
- `imago build` は `[[dependencies]]` がある場合、`imago.lock(version=1)` の `wit_*` / `component_*` / `wit_packages` を検証し、不一致時は `imago update` を要求して失敗する（`kind=wasm` で `component` 未指定の場合は `wit` 由来の期待値と照合する）。
- `imago build --env <name>` は `build/manifest.<name>.json` を生成し、`build/manifest.json` は更新しない。
- `imago build` は `main` で指定された wasm を `build/<sha256>-<name>.wasm` へ materialize し、manifest には manifest ファイル同階層基準の相対パス（`<sha256>-<name>.wasm`）を書き込む。
- `[[bindings]]` は `manifest.bindings[]` へ正規化し、runtime の呼び出し認可入力として扱う。
- `type="http"` のときのみ `[http].port` / `[http].max_body_bytes` を受理し、`manifest.http.port` / `manifest.http.max_body_bytes` へ反映する。
- `type="socket"` のときのみ `[socket].protocol` / `[socket].direction` / `[socket].listen_addr` / `[socket].listen_port` を受理し、`manifest.socket.*` へ反映する。
- CLI の `name` 検証は `imagod` と同等に `..` を拒否し、path 文字を明示的に弾く。
- `--env <name>` は manifest 出力先と `.env.<name>` 解決の双方で同一バリデーションを適用し、path traversal を拒否する。
- `target.<name>.ca_cert` / `client_cert` / `client_key` は path traversal と不正区切りを拒否し、相対指定を `project_root` 基準の絶対パスへ解決する。
- `imagod.storage_root` の既定値は OS 別（Linux=`/var/lib/imago`, macOS=`/usr/local/var/imago`, Windows=`C:\ProgramData\imago`, その他=`/var/lib/imago`）にし、ビルド時環境変数 `IMAGOD_STORAGE_ROOT_DEFAULT` で上書きできる。`imagod.toml` の明示値を最優先する。
- `restart` はトップレベルキーのみ受理し、`runtime.restart_policy` は移行エラーにする。
- `.imago_transitive` は廃止し、transitive WIT package の検証正本を `imago.lock.[[wit_packages]]` へ移行した。`imago build` / `imago deploy` は lock version 1 のみ受理する。

## `target.<name>` の接続キー（deploy 通信）

`imago deploy` / `imago run` / `imago stop` / `imago logs` は `target.<name>` から下記キーを読む。

- `remote`: `host` または `host:port`（`https://` 省略可）
  - IPv6 は `::1`, `[::1]`, `[::1]:4443`, `https://[::1]:4443` を許可
- `server_name`: TLS SNI で利用するサーバ名（省略時は `remote` 側の host）
- `ca_cert`: サーバ証明書検証用 CA PEM
- `client_cert`: mTLS クライアント証明書 PEM
- `client_key`: mTLS クライアント秘密鍵 PEM
  - 上記 3 つは絶対パスまたは `project_root` 相対パスを受理する。
  - 相対パスは `project_root` 基準で解決する。
  - `..`、`\`、Windows ドライブプレフィックス（`C:` など）を含む値は拒否する。

`imago build` が生成する `manifest.target` には、上記のうち `remote` と `server_name` のみを含める。

ローカル検証用の証明書一式は `imago certs generate` で生成できる。
生成先ディレクトリには `.gitignore`（`*` / `!.gitignore`）も作成される。

## imagod 設定ファイル

`imagod` は `imagod.toml` を読む。既定パスは `/etc/imago/imagod.toml`。

- `listen_addr`
- `storage_root`
- `server_version`
- `compatibility_date`（`YYYY-MM-DD`、既定値 `2026-02-10`）
- `tls.server_cert`
- `tls.server_key`
- `tls.client_ca_cert`
- `runtime.chunk_size`
- `runtime.max_inflight_chunks`
- `runtime.max_artifact_size_bytes`（既定 `67108864` = 64 MiB）
- `runtime.upload_session_ttl_secs`
- `runtime.stop_grace_timeout_secs`（既定 `30`）
- `runtime.runner_ready_timeout_secs`（既定 `3`）
- `runtime.runner_log_buffer_bytes`（既定 `262144`）
- `runtime.epoch_tick_interval_ms`（既定 `50`）

`storage_root` の既定値決定順序:

1. `imagod.toml` の `storage_root` 明示値
2. ビルド時環境変数 `IMAGOD_STORAGE_ROOT_DEFAULT`（空文字は無効）
3. OS 別既定値
   - Linux: `/var/lib/imago`
   - macOS: `/usr/local/var/imago`
   - Windows: `C:\ProgramData\imago`
   - その他: `/var/lib/imago`

`imagod` の runtime 検証制約:

- `runtime.chunk_size`: `1..=8388608`（1 byte 以上 8 MiB 以下）
- `runtime.max_inflight_chunks`: `1` 以上
- `runtime.max_artifact_size_bytes`: `1` 以上
- `runtime.stop_grace_timeout_secs`: `1` 以上
- `runtime.runner_ready_timeout_secs`: `1` 以上
- `runtime.runner_log_buffer_bytes`: `1` 以上
- `runtime.epoch_tick_interval_ms`: `1` 以上
