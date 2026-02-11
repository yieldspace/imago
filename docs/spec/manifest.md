# Manifest Specification (`build/manifest.json`)

## 目的

`build/manifest.json` の固定フォーマットを定義し、CLI と runtime の入力仕様を一致させる。

関連仕様:

- 設定入力の正規化: [`config.md`](./config.md)
- 転送・実行仕様: [`deploy-protocol.md`](./deploy-protocol.md)

## 出力場所

- 固定パス: `build/manifest.json`
- `imago build --env <name>` 時: `build/manifest.<name>.json`

<a id="required-fields"></a>
## 必須フィールド

| フィールド | 型 | 説明 |
|---|---|---|
| `name` | string | サービス名 |
| `main` | string | Wasm エントリパス |
| `type` | string | `cli` / `http` / `socket` |
| `target` | object | 解決済みターゲット設定 |
| `vars` | object | env 反映後の公開変数 |
| `secrets` | object | env 反映後の secret 値 |
| `assets` | array | 同梱アセット一覧 |
| `dependencies` | array | 依存解決結果 |
| `hash` | object | 全体整合性情報 |

## `hash` フィールド

`hash` は以下を必須とする。

- `algorithm`: `sha256`
- `value`: 計算済み digest
- `targets`: `wasm`, `manifest`, `assets`

<a id="hash-targets"></a>
## ハッシュ対象ルール

全体 hash は次を対象に計算する。

1. `app.wasm` のバイト列
2. `manifest.json` の正規化 JSON バイト列
3. `assets` 配下ファイルのバイト列（パス昇順）

`hash.targets` に `wasm` / `manifest` / `assets` が揃っていない場合は不正 manifest とみなす。

<a id="secret-bundling"></a>
## secret 同梱方針

- `secrets` は manifest に同梱してデプロイ時に送信する。
- runtime 側は `secrets` をログへ出力してはいけない。
- CLI 側は `--dry-run` を除き `secrets` の実値を表示してはいけない。

## 正常例と異常例

- 正常例: [`examples/manifest.valid.json`](./examples/manifest.valid.json)
- 異常例（必須欠落）: [`examples/manifest.invalid.missing-required.json`](./examples/manifest.invalid.missing-required.json)
- 異常例（型不正）: [`examples/manifest.invalid.bad-type.json`](./examples/manifest.invalid.bad-type.json)
- 異常例（hash 検証不一致）: [`examples/manifest.invalid.hash-mismatch.json`](./examples/manifest.invalid.hash-mismatch.json)
- 異常例（secret 形式不正）: [`examples/manifest.invalid.secret-shape.json`](./examples/manifest.invalid.secret-shape.json)

## バリデーション要件

- 必須フィールド欠落は拒否。
- `type` が定義外なら拒否。
- `hash.algorithm != "sha256"` は拒否。
- `hash.targets` が不足または重複なら拒否。
- `secrets` は key-value オブジェクトのみ許可。

## 実装ノート

- manifest は deploy リクエストの入力正本とする。
- runtime 側での再計算結果が `hash.value` と一致しない場合は整合性エラーとして扱う。

## 実装反映ノート

- CLI の `hash.value` 計算は `hash.value` を空文字にした中間 manifest JSON を使って実行する。
  - 連結順序は `main`（wasm bytes）→ 中間 manifest JSON bytes → assets bytes（`path` 昇順）。
- CLI は `main` の実体 wasm を `build/<sha256>-<name>.wasm` へ配置し、`manifest.main` はこの materialize 済みファイルを指す。
- `hash.value` の wasm 対象は `manifest.main` が指す materialize 後ファイルとする。
