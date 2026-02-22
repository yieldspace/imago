# imago Specification

このディレクトリは imago の仕様正本です。仕様は「抽象から具体へ」の順で読めるように構成します。

## 読み順（抽象 → 具体）

### Layer 0: 全体方針
- 全体像と前提: [`README.md`](./README.md)

### Layer 1: 外部契約
- 設定仕様: [`config.md`](./config.md)
- マニフェスト仕様: [`manifest.md`](./manifest.md)
- デプロイ通信仕様: [`deploy-protocol.md`](./deploy-protocol.md)
- 観測・状態照会仕様: [`observability.md`](./observability.md)
- CLI 出力仕様: [`cli-output.md`](./cli-output.md)

### Layer 2: サブシステム概要
- `imagod` 概要: [`imagod.md`](./imagod.md)
- `imago-protocol` 概要: [`imago-protocol.md`](./imago-protocol.md)

### Layer 3: 実装詳細
- `imagod` 内部詳細: [`imagod-internals.md`](./imagod-internals.md)
- `imago-protocol` 内部詳細: [`imago-protocol-internals.md`](./imago-protocol-internals.md)

### 具体例
- examples 一覧: [`examples/README.md`](./examples/README.md)
- サンプル JSON: [`examples/`](./examples/)

## 適用範囲

- MVP の実装判断をなくすための最小仕様を定義する。
- 対象は `imago.toml`、`build/manifest.json`、deploy protocol、command stream 観測性、CLI 出力契約、`imagod`/`imago-protocol` の責務と内部構造。
- `logs` は `logs.request`（stream）+ `logs.chunk`/`logs.end`（DATAGRAM）の混在契約として扱う。
- 実装コードより仕様を優先する。

## 共通前提

- 通信方式は QUIC + WebTransport + CBOR。
- 認証は RPK + TOFU（初回接続で `~/.imago/known_hosts` へ鍵 pin）。
- `hello.negotiate` の互換キーは `compatibility_date`。
- `ProtocolEnvelope` の `request_id` / `correlation_id` は UUID。
- `state.request` の応答メッセージ種別は `state.response`。
- 観測イベントは永続保存せず、再送しない。
- `logs` 本文の DATAGRAM 配信は欠損許容で、`seq` による検知のみ行う。

## 実装反映ノート運用

設計差分（プロトコル契約、既定値、バリデーション、エラー契約）の既定記録先は、該当する既存仕様ファイルの `## 実装反映ノート` とする。
記載量が大きい場合のみ別ファイル化し、このページと該当仕様から参照リンクを張る。

## 非対象

- blue-green デプロイ
- 差分配信
- 監視ダッシュボード UI
- メトリクスの詳細仕様

## 実装反映ノート（RPK + TOFU / 2026-02-18）

- [BREAKING] deploy 通信の認証前提を mTLS/X.509 から RPK + TOFU へ更新した。
- `target.<name>` は `client_key` を使い、`known_hosts` は CLI 既定 `~/.imago/known_hosts` 固定運用にした（`ca_cert` / `client_cert` / `known_hosts` は廃止）。
- `imagod.toml` の TLS 設定は `server_key` と `client_public_keys`（ed25519 公開鍵 raw 32byte hex allowlist）を正本にする。

## 実装反映ノート（Network RPC / 2026-02-18）

- [BREAKING] `imago` CLI から `--env` を廃止し、`[env.*]` と `.env.<name>` の解決を削除した。
- [BREAKING] manifest/config の `bindings` は `target` から `name` へ移行した。
- `rpc.invoke` を protocol に追加し、local/remote RPC の payload を CBOR で統一した。

## 実装反映ノート（env 統合 / 2026-02-22）

- [BREAKING] `manifest` の `vars` / `secrets` を削除し、環境変数は `manifest.wasi.env` に統一した。
- `imago build` は `project_root/.env` を `manifest.wasi.env` へ統合し、同名キーは `.env` を優先する。
