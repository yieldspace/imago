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
| `name` | string | 1-63 文字、空文字不可 | サービス識別名 |
| `main` | string | 相対パス、空文字不可 | 実行対象の Wasm パス |
| `type` | string | `cli` / `http` / `socket` のいずれか | 実行モデル |
| `target` | table | 必須 | デプロイ先設定 |

`name` の推奨文字集合は英数字、`-`、`_`。実装は不正文字を明確なエラーで拒否する。

## 推奨キー

- `args`
- `capabilities`
- `limits`
- `runtime`
- `vars`
- `assets`
- `dependencies`

<a id="env-override"></a>
## `--env` 上書き規則

1. `--env` 未指定時は base 設定のみを使う。
2. `--env <name>` 指定時は `[env.<name>]` を base 設定にマージする。
3. `--env <name>` 指定時に読み込む環境変数ファイルは `.env.<name>` のみ。
4. マージ範囲は設定全体。競合時は env 側を優先する。
5. 指定された env 名が存在しない場合はエラー。

<a id="capability-model"></a>
## 権限モデル

### 既定挙動

- `capabilities` 未指定時は全拒否（deny-by-default）。

### `capabilities`

- `capabilities.fs`: 許可するファイルシステムアクセス。
- `capabilities.net`: 許可するネットワークアクセス。
- `capabilities.dev`: `/dev` 配下の許可デバイス。

### `privileged`

- `privileged = true` の場合、`capabilities` は無視し全許可。
- `privileged` 未指定時は `false` として扱う。

<a id="defaults"></a>
## 既定値

| キー | 既定値 | 備考 |
|---|---|---|
| `limits.shutdown_timeout` | `30s` | graceful 停止待ち時間 |
| `runtime.restart_policy` | `never` | MVP では詳細パラメータを固定しない |

## バリデーション要件

- 必須キー欠落はエラー。
- `type` 不正値はエラー。
- `main` が存在しない場合はビルド時エラー。
- `shutdown_timeout` が 0 以下はエラー。
- `privileged = true` かつ `capabilities` 指定ありでもエラーにはしない（`capabilities` を無視）。

## 異常系

- 存在しない env 指定。
- `.env.<name>` の読み込み失敗。
- 型不正（例: `shutdown_timeout = "abc"`）。
- 不正な `type`。

## 実装ノート

- 設定ロードは CLI 側で厳格検証し、正規化結果を [`manifest.md`](./manifest.md) の形式で出力する。
- runtime 側は manifest を信頼入力として扱い、再解釈を最小化する。

## `target.<name>` の接続キー（deploy 通信）

`imago deploy` は `target.<name>` から下記キーを読む。

- `remote`: `host` または `host:port`（`https://` 省略可）
  - IPv6 は `::1`, `[::1]`, `[::1]:4443`, `https://[::1]:4443` を許可
- `server_name`: TLS SNI で利用するサーバ名（省略時は `remote` 側の host）
- `ca_cert`: サーバ証明書検証用 CA PEM
- `client_cert`: mTLS クライアント証明書 PEM
- `client_key`: mTLS クライアント秘密鍵 PEM

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
- `runtime.epoch_tick_interval_ms`（既定 `50`）

`imagod` の runtime 検証制約:

- `runtime.chunk_size`: `1..=8388608`（1 byte 以上 8 MiB 以下）
- `runtime.max_inflight_chunks`: `1` 以上
- `runtime.max_artifact_size_bytes`: `1` 以上
- `runtime.stop_grace_timeout_secs`: `1` 以上
- `runtime.epoch_tick_interval_ms`: `1` 以上
