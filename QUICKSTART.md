# QUICKSTART

## Goal

This guide helps you:

1. Set up the required toolchain and Imago CLI.
2. Initialize `imago.toml`.
3. Run `examples/local-imagod` and confirm the service starts.

Run commands from the repository root (`imago/`) unless a step says otherwise.

## Prerequisites

- Rust toolchain
- `wasm32-wasip2` target:

```bash
rustup target add wasm32-wasip2
```

## Install Imago CLI

Choose one installation method:

Option A:
```bash
curl -sSf https://install.imago.sh | sh
```

Option B:
```bash
cargo install imago-cli --git https://github.com/yieldspace/imago
```

## Clone Repository

```bash
git clone https://github.com/yieldspace/imago
cd imago
```

## Initialize Project

```bash
# From the repository root, interactive (TTY)
imago project init .

# From the repository root, non-interactive (CI/no TTY): --lang is required
imago project init services/example --lang rust
```

`imago project init` updates `.gitignore` in the project directory and ensures `.imago` and `/build` entries exist.

## Run Local Example

```bash
# Terminal 1
# Start daemon
cd examples/local-imagod
cargo run -p imagod -- --config imagod.toml
```

```bash
# Terminal 2
# Deploy and stream logs
cd examples/local-imagod
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-app --tail 200
```

## Success Check

After running the log command, confirm output includes a line similar to:

```text
local-imagod-app stdout | local-imagod-app started
```

## Next Steps

- More runnable examples: [examples/README.md](examples/README.md)
- Documentation index: [docs/README.md](docs/README.md)
- Generate API docs:

```bash
cargo doc --workspace --no-deps
```
