# imago-compose bindings example

## 構成

- 2 service 構成: `rpc-greeter` (`type = "rpc"`), `cli-client` (`type = "cli"`)
- `cli-client` が `rpc-greeter` の `acme:clock/api.now` を呼び出します。

## ローカル 1 ノード手順

1. ターミナル A で build/update と `imagod` 起動を行います。

```bash
cd examples/imago-compose-bindings
cargo run -p imago-cli -- compose build prepare --target default
cargo run -p imago-cli -- compose update dev
cargo run -p imago-cli -- compose build dev --target default
cargo run -p imagod -- --config imagod.toml
```

2. ターミナル B で deploy とログ確認を行います。

```bash
cd examples/imago-compose-bindings
cargo run -p imago-cli -- compose deploy dev --target default
cargo run -p imago-cli -- compose logs dev --target default --name cli-client --tail 200
```

## Docker cross-imagod 手順（alice/bob）

1. `imagod-alice` / `imagod-bob` / `imago-deployer` を起動します。

```bash
cd examples/imago-compose-bindings/docker
docker compose --project-name imago-compose-bindings-alice-bob-e2e up --build -d imagod-alice imagod-bob imago-deployer
```

2. `imago-deployer` 内で `cargo run ... -p imago-cli -- compose ...` を実行し、`greeter -> bob`、`client -> alice` の順で deploy します。

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose update greeter
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose build greeter --target bob
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose deploy greeter --target bob

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose update client
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose build client --target alice
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose deploy client --target alice
```

3. cert 配布前の失敗ログを確認し、binding cert を配布後に成功ログを確認します。

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose logs client --target alice --name cli-client --tail 200

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- bindings cert deploy --from imagod-alice:4443 --to imagod-bob:4443

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  cargo run -p imago-cli -- compose logs client --target alice --name cli-client --tail 200
```

必要なら最後に `docker compose --project-name imago-compose-bindings-alice-bob-e2e down --remove-orphans` で停止できます。

## 成功判定

- ローカル 1 ノード: `compose logs ... --name cli-client` に `acme:clock/api.now =>` が含まれる。
- Docker cross-imagod: cert 配布前は接続失敗ログ、`bindings cert deploy` 後に `acme:clock/api.now =>` が含まれる。

## Troubleshooting

- `localhost:4443` の TOFU pin 不整合で接続失敗する場合のみ、`$HOME/.imago/known_hosts` から該当エントリ（`localhost:4443` / `127.0.0.1:4443`）を削除して再実行してください。
