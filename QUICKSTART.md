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
./examples/local-imagod/scripts/quickstart.sh
```

## 成功判定

`command.event` が `succeeded` で終われば成功です。

## 他のexamples参照

他の実行例は [`examples/README.md`](examples/README.md) を参照してください。
