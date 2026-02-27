# QUICKSTART

## Prerequisites

- Rust toolchain
- `wasm32-wasip2` target

```bash
rustup target add wasm32-wasip2
```

- Imago CLI

```bash
curl -sSf https://install.imago.sh | sh
```

```bash
cargo install imago-cli --git https://github.com/yieldspace/imago
```

## Clone Repository

```bash
git clone https://github.com/yieldspace/imago
cd imago
```

## Initialize `imago.toml`

```bash
# Interactive (TTY)
imago project init .

# Non-interactive (CI/no TTY): --lang is required
imago project init services/example --lang rust
```

`imago project init` updates `.gitignore` in the project directory and ensures `.imago` and `/build` entries exist.

## Run Example

```bash
# Terminal 1
cd examples/local-imagod
cargo run -p imagod -- --config imagod.toml
```

```bash
# Terminal 2
cd examples/local-imagod
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-app --tail 200
```

## Success Check

In rich/plain output, success includes a log line similar to:

```text
local-imagod-app stdout | local-imagod-app started
```

## More Examples

See [examples/README.md](examples/README.md).

## Documentation And Code References

- Documentation index: [docs/README.md](docs/README.md)
- Generated API docs:

```bash
cargo doc --workspace --no-deps
```
