# imago-compose bindings example

この example は 2 service 構成です。

1. `rpc-greeter` (`type = "rpc"`)
2. `cli-client` (`type = "cli"`)

`rpc-greeter` は現在時刻（UNIX 秒）を返す `acme:clock/api.now` を export し、`cli-client` はそれを 5 秒おきに呼び出します。

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

## Docker Compose 2-node 実行方法（alice/bob）

`docker/` 配下には `imagod-alice` / `imagod-bob` と、deploy 実行用の `imago-deployer`（`imago-cli` 実行コンテナ）を含む構成が入っています。
このディレクトリで次を実行してください。

```bash
./scripts/test-e2e-docker-cross-imagod.sh
```

`test-e2e-docker-cross-imagod.sh` は次を一括実行します。

1. `docker/certs/control` / `docker/certs/alice` / `docker/certs/bob` の鍵生成
2. `docker/imagod-alice.toml` / `docker/imagod-bob.toml` の `tls.admin_public_keys` / `tls.client_public_keys` 更新
3. `docker compose up --build -d imagod-alice imagod-bob imago-deployer` で 3 service を起動
4. `imago-deployer` コンテナ内で `imago-cli` をビルドし、`known_hosts` を `imagod-alice:4443` / `imagod-bob:4443` で初期化
5. `imago-deployer` コンテナ内で `imago compose update/build/deploy` を profile ごとに実行
- `greeter` profile を `--target bob` で deploy（`rpc-greeter` を bob に配置）
- `client` profile を `--target alice` で deploy（`cli-client` を alice に配置）
6. `imago-deployer` コンテナ内で `imago compose logs client --target alice --name cli-client --tail 200` を確認し、
`imago:node/rpc connection failed:` または `acme:clock/api.now failed:`（証明書配布前の失敗）を検証
7. `imago-deployer` コンテナ内で `imago bindings cert deploy --from imagod-alice:4443 --to imagod-bob:4443`
8. 再度 `imago compose logs client --target alice --name cli-client --tail 200` を確認し、
`acme:clock/api.now =>`（証明書配布後の成功）を検証

スクリプトは成功・失敗に関わらず `docker compose down --remove-orphans` を実行します。

`docker compose` は host bind mount に依存せず、`Dockerfile.imagod` で `docker/` 配下の config/certs をイメージへ取り込みます。deploy 自体は `imago-deployer` から同一 compose network 内で実行するため、ホスト側の port forward なしで検証可能です。

## 構成メモ

- `imago-compose.toml`
- `profile.prepare -> compose.prepare`（`rpc-greeter` の先行ビルド用）
- `profile.dev -> compose.devstack`
- `target.default` に `remote/server_name/client_key` を集約
- 実行順序: `rpc-greeter` -> `cli-client`
- `docker/imago-compose.toml`
- `profile.greeter -> compose.greeter`（`rpc-greeter` を bob へ deploy）
- `profile.client -> compose.client`（`cli-client` を alice へ deploy）
- `target.alice` / `target.bob` は `imagod-alice:4443` / `imagod-bob:4443` を向く
- `docker/docker-compose.yml`
- `imago-deployer` が `imago-cli` 実行基盤を提供し、deploy テストを compose 内で完結させる
- `services/cli-client`
- `[[bindings]] name="rpc-greeter" wit="file://../rpc-greeter/wit/world.wit"`
- `imago update` 実行時に `wit` source から package 内の全 interface を展開し、`manifest.bindings` へ `<package>/<interface>` を出力する
- 各 service の `imago.toml`
- `target.*` は持たず、compose 側 target を利用
