# imago-compose bindings example

## 構成

- 2 service 構成: `rpc-greeter` (`type = "rpc"`), `cli-client` (`type = "cli"`)
- `cli-client` が `rpc-greeter` の `acme:clock/api.now` を呼び出します。

## ローカル 1 ノード手順

`stack deploy` / `stack logs` は `ssh://localhost?...` 経由で `imagod proxy-stdio` を呼びます。
事前に `ssh localhost true` が対話なしで成功し、SSH ログインシェルの `PATH` から `imagod` を実行できる状態にしてください。

1. ターミナル A で build/sync と `imagod` 起動を行います。

```bash
cd examples/imago-compose-bindings
cargo run -p imago-cli -- stack build prepare --target default
cargo run -p imago-cli -- stack sync dev
cargo run -p imago-cli -- stack build dev --target default
cargo run -p imagod -- --config imagod.toml
```

2. ターミナル B で stack deploy とログ確認を行います。

```bash
cd examples/imago-compose-bindings
cargo run -p imago-cli -- stack deploy dev --target default
cargo run -p imago-cli -- stack logs dev --target default --name cli-client --tail 200
```

## Docker cross-imagod 手順（alice/bob）

1. `imagod-alice` / `imagod-bob` / `imago-deployer` を起動します。

```bash
cd examples/imago-compose-bindings/docker
docker compose --project-name imago-compose-bindings-alice-bob-e2e up --build -d imagod-alice imagod-bob imago-deployer
```

2. `imago-deployer` 内で `cargo run ... -p imago-cli -- stack ...` を実行し、`greeter -> bob`、`client -> alice` の順で stack deploy します。

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack sync greeter
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack build greeter --target bob
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack deploy greeter --target bob

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack sync client
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack build client --target alice
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack deploy client --target alice
```

3. cert 配布前の失敗ログを確認し、trust cert を配布後に成功ログを確認します。

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack logs client --target alice --name cli-client --tail 200

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- trust cert replicate \
    --from ssh://imagod-alice?socket=/run/imago/imagod.sock \
    --from-authority rpc://imagod-alice:4443 \
    --to ssh://imagod-bob?socket=/run/imago/imagod.sock \
    --to-authority rpc://imagod-bob:4443

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- stack logs client --target alice --name cli-client --tail 200
```

必要なら最後に `docker compose --project-name imago-compose-bindings-alice-bob-e2e down --remove-orphans` で停止できます。

Docker compose 例の SSH 制御鍵と `known_hosts` は、起動時に compose の shared volume 上で自動生成されます。`imago-deployer` は `imagod-alice` / `imagod-bob` に限定した `known_hosts` を使い、`Host *` 無効化は行いません。

## 成功判定

- ローカル 1 ノード: `stack logs ... --name cli-client` に `acme:clock/api.now =>` が含まれる。
- Docker cross-imagod: cert 配布前は接続失敗ログ、`trust cert replicate` 後に `acme:clock/api.now =>` が含まれる。

## Troubleshooting

- ローカル 1 ノードで失敗する場合は `ssh localhost true` と `imagod proxy-stdio --socket /tmp/imagod-compose-bindings.sock` を同じユーザーで確認してください。
- Docker cross-imagod では `imago-deployer` から `ssh imagod-alice true` / `ssh imagod-bob true` が通ることを先に確認してください。
