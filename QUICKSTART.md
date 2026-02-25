# QUICKSTART

## 前提

- Rust toolchain
- `wasm32-wasip2` target

```bash
rustup target add wasm32-wasip2
```

- imago CLI

```bash
curl -sSf https://imago.yield.space | sh
```

```bash
cargo install imago
```

## リポジトリ取得

```bash
git clone https://github.com/yieldspace/imago
cd imago
```

## `imago.toml` 初期化

```bash
# 対話実行（TTY）
imago init .

# 非対話実行（CI/--json/TTYなし）は --lang が必須
imago init services/example --lang rust
imago --json init services/example --lang generic
```

`imago init` は `imago.toml` 作成先の `.gitignore` を自動整備し、
`.imago` と `/build` を不足分だけ追記します（`.gitignore` が無ければ作成）。

## 実行

```bash
# ターミナル1
cd examples/local-imagod
cargo run -p imagod -- --config imagod.toml
```

```bash
# ターミナル2
cd examples/local-imagod
# ターミナル1 で imagod が起動したことを確認してから実行
cargo run -p imago-cli -- deploy --target default --detach
cargo run -p imago-cli -- logs local-imagod-app --tail 200
```

## 成功判定

既定（Rich）または `CI=true`（Plain）では、`imago-cli logs` の出力に次の形式で `local-imagod-app started` が含まれていれば成功です。

```text
local-imagod-app stdout | local-imagod-app started
```

`--json` 指定時は JSON Lines で `log.line` が出力されます（`logs --json` は `command.summary` を出力しません）。

```bash
cargo run -p imago-cli -- --json logs local-imagod-app --tail 200
```

```json
{"type":"log.line","name":"local-imagod-app","stream":"stdout","timestamp":"1739982001","log":"local-imagod-app started"}
```

失敗時のみ `command.error` が 1 行出力されます。

```json
{"type":"command.error","command":"logs","message":"...","stage":"logs","code":"E_UNKNOWN"}
```

## 他のexamples参照

他の実行例は [`examples/README.md`](examples/README.md) を参照してください。
