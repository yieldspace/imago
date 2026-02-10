# imago Specification

このディレクトリは imago の仕様正本です。実装判断が必要な項目は、この配下だけで完結するように定義します。

## 読み方

- 全体像と前提: このページ
- 設定契約: [`config.md`](./config.md)
- マニフェスト契約: [`manifest.md`](./manifest.md)
- デプロイ通信契約: [`deploy-protocol.md`](./deploy-protocol.md)
- 追跡と観測性契約: [`observability.md`](./observability.md)
- 具体例: [`examples/`](./examples/)

## 適用範囲

- MVP の実装判断をなくすための最小仕様を定義する。
- 対象は `imago.toml`、`build/manifest.json`、デプロイプロトコル、operation 追跡契約。
- 実装コードより仕様を優先する。

## 共通前提

- 通信方式は QUIC + WebTransport + CBOR。
- 認証は mTLS。
- デプロイ失敗時の `auto_rollback` 既定値は `true`。
- operation 履歴保持期間は 24 時間。
- 仕様間の参照はリンクで行い、重複説明を避ける。

## 仕様の境界

- 設定キーの意味と既定値は [`config.md`](./config.md) が正本。
- `build/manifest.json` のフォーマットは [`manifest.md`](./manifest.md) が正本。
- リクエスト/レスポンス契約は [`deploy-protocol.md`](./deploy-protocol.md) が正本。
- `operation.watch` のイベント契約は [`observability.md`](./observability.md) が正本。

## 非対象

- blue-green デプロイ。
- 差分配信。
- 監視ダッシュボード UI。
- メトリクスの詳細仕様。
