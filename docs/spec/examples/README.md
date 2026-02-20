# Examples (`docs/spec/examples`)

このディレクトリは仕様確認用の最小 JSON 例です。値は短いダミー値に寄せ、ケース判定に必要な構造のみ残します。

| ファイル | 用途 | 主な参照先 |
|---|---|---|
| `manifest.valid.json` | 正常な `manifest`（`socket`/`vars`/`secrets`/`assets`/`hash` を含む） | [`manifest.md`](../manifest.md), [`deploy-protocol.md`](../deploy-protocol.md) |
| `manifest.invalid.missing-required.json` | 必須フィールド欠落（`type`/`target` 欠落） | [`manifest.md`](../manifest.md) |
| `manifest.invalid.bad-type.json` | `type` が定義外値（`worker`） | [`manifest.md`](../manifest.md) |
| `manifest.invalid.hash-mismatch.json` | `hash` の検証不一致ケース | [`manifest.md`](../manifest.md) |
| `manifest.invalid.secret-shape.json` | `secrets` が object ではない形（配列） | [`manifest.md`](../manifest.md) |
| `rpc.invoke.request.json` | `rpc.invoke` リクエスト envelope | [`imago-protocol.md`](../imago-protocol.md), [`deploy-protocol.md`](../deploy-protocol.md) |
| `rpc.invoke.response.success.json` | `rpc.invoke` 成功レスポンス（`result_cbor`） | [`imago-protocol.md`](../imago-protocol.md), [`deploy-protocol.md`](../deploy-protocol.md) |
| `rpc.invoke.response.error.json` | `rpc.invoke` エラーレスポンス（`payload.error`） | [`imago-protocol.md`](../imago-protocol.md), [`deploy-protocol.md`](../deploy-protocol.md) |
