# imago-compose bindings example

この example は 2 service 構成です。

1. `rpc-greeter` (`type = "rpc"`)
2. `cli-client` (`type = "cli"`)

`rpc-greeter` は現在時刻（UNIX 秒）を返す `acme:clock/api.now` を export し、`cli-client` はそれを 5 秒おきに `rpc.local()` 経由で呼び出します。

## 実行方法（推奨）

このディレクトリで次を実行してください。

```bash
./scripts/test-e2e.sh
```

`test-e2e.sh` は次を一括実行します。

1. `imago certs generate` による admin 用 / rpc 用鍵生成
2. `imagod.toml` の `tls.admin_public_keys` / `tls.client_public_keys` の更新
3. `imago compose build prepare --target default`（`rpc-greeter` 先行ビルド）
4. `imago compose update dev`
5. `imago compose build dev --target default`
6. `imagod --config ./imagod.toml` の起動
7. `imago compose deploy dev --target default`
8. `imago compose logs dev --target default --name cli-client --tail 200` で `acme:clock/api.now =>` を検証

スクリプトは成功・失敗に関わらず `imagod` を停止します。

## 構成メモ

- `imago-compose.toml`
  - `profile.prepare -> compose.prepare`（`rpc-greeter` の先行ビルド用）
  - `profile.dev -> compose.devstack`
  - `target.default` に `remote/server_name/client_key` を集約
  - 実行順序: `rpc-greeter` -> `cli-client`
- `services/cli-client`
  - `[[bindings]] name="rpc-greeter" wit="acme:clock/api"`
  - `imago update` 実行時に `wit/deps` を自動生成
- 各 service の `imago.toml`
  - `target.*` は持たず、compose 側 target を利用
