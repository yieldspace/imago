# QUICKSTART

## Prerequisites

- Rust toolchain
- `wasm32-wasip2` target

```bash
rustup target add wasm32-wasip2
```

- Imago CLI

```bash
curl -sSf https://imago.yield.space | sh
```

```bash
cargo install imago
```

## Clone Repository

```bash
git clone https://github.com/yieldspace/imago
cd imago
```

## Initialize `imago.toml`

```bash
# Interactive (TTY)
imago init .

# Non-interactive (CI/--json/no TTY): --lang is required
imago init services/example --lang rust
imago --json init services/example --lang generic
```

`imago init` updates `.gitignore` in the project directory and ensures `.imago` and `/build` entries exist.

## Run Example

```bash
# Terminal 1
cd examples/local-imagod
cargo run -p imagod -- --config imagod.toml
```

```bash
# Terminal 2
cd examples/local-imagod
cargo run -p imago-cli -- deploy --target default --detach
cargo run -p imago-cli -- logs local-imagod-app --tail 200
```

## Success Check

In rich/plain output, success includes a log line similar to:

```text
local-imagod-app stdout | local-imagod-app started
```

For JSON mode:

```bash
cargo run -p imago-cli -- --json logs local-imagod-app --tail 200
```

```json
{"type":"log.line","name":"local-imagod-app","stream":"stdout","timestamp":"1739982001","log":"local-imagod-app started"}
```

Failure emits `command.error`:

```json
{"type":"command.error","command":"logs","message":"...","stage":"logs","code":"E_UNKNOWN"}
```

## More Examples

See [examples/README.md](examples/README.md).
