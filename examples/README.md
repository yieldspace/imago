# examples

## 一覧

| ディレクトリ | 概要 | 実行コマンド |
| --- | --- | --- |
| `examples/local-imagod` | `imago deploy` の最小構成 | `cd examples/local-imagod && ./scripts/quickstart.sh` |
| `examples/local-imagod-http` | `type=http` の実行例 | `cd examples/local-imagod-http && ./scripts/run-imagod.sh`（別ターミナル: `./scripts/deploy.sh && ./scripts/verify-http.sh`） |
| `examples/local-imagod-socket` | `type=socket` の実行例 | `cd examples/local-imagod-socket && ./scripts/run-imagod.sh`（別ターミナル: `./scripts/deploy.sh`） |
| `examples/local-imagod-plugin-hello` | Wasm plugin 依存の実行例 | `cd examples/local-imagod-plugin-hello && cargo run --manifest-path ../../Cargo.toml -p imago-cli -- update && ./scripts/run-imagod.sh`（別ターミナル: `./scripts/deploy.sh && ./scripts/verify-hello.sh`） |
| `examples/local-imagod-plugin-native-admin` | native plugin 依存の実行例 | `cd examples/local-imagod-plugin-native-admin && cargo run --manifest-path ../../Cargo.toml -p imago-cli -- update && ./scripts/run-imagod.sh`（別ターミナル: `./scripts/deploy.sh && ./scripts/verify-admin.sh`） |
| `examples/imago-compose-bindings` | compose/bindings の実行例 | `cd examples/imago-compose-bindings && ./scripts/test-e2e.sh` / `./scripts/test-e2e-docker-cross-imagod.sh` |
