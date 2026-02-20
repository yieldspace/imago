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
cargo run -p imago-cli -- deploy --target default
cargo run -p imago-cli -- logs local-imagod-app --tail 200
```

## 成功判定

`imago-cli logs` の出力に `local-imagod-app started` が含まれていれば成功です。

## 他のexamples参照

他の実行例は [`examples/README.md`](examples/README.md) を参照してください。
