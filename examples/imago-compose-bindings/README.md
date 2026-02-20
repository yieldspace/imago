# imago-compose bindings example

## 構成

- 2 service 構成: `rpc-greeter` (`type = "rpc"`), `cli-client` (`type = "cli"`)
- `cli-client` が `rpc-greeter` の `acme:clock/api.now` を呼び出します。

## 実行

```bash
./scripts/test-e2e.sh
```

```bash
./scripts/test-e2e-docker-cross-imagod.sh
```

どちらもこの example の実行スクリプトです。

## 確認

- `imago compose logs ... --name cli-client` の出力に `acme:clock/api.now =>` が含まれれば成功です。
