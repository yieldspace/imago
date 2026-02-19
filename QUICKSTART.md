# QUICKSTART

この Quickstart は、`warg://sizumita:ferris@0.1.0` を依存として読み込み、
`sizumita:ferris/says.say` を呼び出す現行仕様の最短フローです。

## Install CLI

```bash
curl -sSf https://imago.yield.space | sh
```

From cargo:

```bash
cargo install imago
```

## 事前条件

- Rust toolchain
- `wasm32-wasip2` target（未導入なら `rustup target add wasm32-wasip2`）

## 1. リポジトリを取得

```bash
git clone https://github.com/yieldspace/imago
cd imago/examples/local-imagod-plugin-hello
```

## 2. 依存解決（必須）

```bash
cargo run --manifest-path ../../Cargo.toml -p imago-cli -- update
```

この時点で以下が行われます。

- `.imago/deps/` に依存WIT/Componentを保存
- `.imago/deps/` から `wit/deps/` を再生成
- `imago.lock (version=1)` に `wit_*` 固定
- transitive package は `imago.lock` の `[[wit_packages]]` に固定（`.imago_transitive` は未使用）
- `wit` が component の場合は `component_*` も自動固定

`examples/local-imagod-plugin-hello/imago.toml` の依存設定は以下です。

```toml
[[dependencies]]
name = "sizumita:ferris"
version = "0.1.0"
kind = "wasm"
wit = "warg://sizumita:ferris@0.1.0"

[capabilities.deps]
"sizumita:ferris" = ["sizumita:ferris/says@0.1.0.say"]
```

`warg://sizumita:ferris@0.1.0` は component のため、`[dependencies.component]` は不要です。

## 3. `imagod` を起動（ターミナル1）

```bash
./scripts/run-imagod.sh
```

## 4. デプロイ（ターミナル2）

```bash
./scripts/deploy.sh
```

`imago build` / `imago deploy` は `.imago/deps/` を依存の正本として使います。
必要な依存キャッシュが不足している場合は失敗し、`imago update` を要求します。

## 5. 出力確認

```bash
./scripts/verify-hello.sh
```

または直接:

```bash
cargo run --manifest-path ../../Cargo.toml -p imago-cli -- logs local-imagod-plugin-hello-app --tail 200
```

`hello from imago` と ferris の出力が見えれば成功です。

注:

- アプリが短時間で終了するため、`imago logs ...` が `NotFound` になる場合があります。
- その場合は `./scripts/run-imagod.sh` を実行しているターミナルの `service log` 出力を確認してください。
