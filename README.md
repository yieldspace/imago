# imago

Imago is a Wasm Component deployment and runtime platform for embedded Linux systems.
It focuses on lightweight execution, explicit capability boundaries, and a predictable remote deploy workflow.

Start with the documentation landing page:

- [Documentation Home](docs/README.md)

## Highlights

- Wasm Component as the deployable unit
- Capability-based permission model
- Consistent deploy/run/stop workflow for local and remote targets
- Embedded Linux oriented runtime footprint

## Quickstart

```bash
curl -sSf https://install.imago.sh | sh
```

```bash
cargo install imago-cli --git https://github.com/yieldspace/imago
```

```bash
git clone https://github.com/yieldspace/imago
cd imago
```

Initialize a project:

```bash
# Interactive (TTY)
imago project init .

# Non-interactive (CI/no TTY)
imago project init services/example --lang rust
```

Run local example:

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

## CLI Output Modes

Mode priority:

1. `CI=true` (plain output)
2. Rich terminal output

## Configuration References

- [imago.toml Reference](docs/imago-configuration.md)
- [imagod.toml Reference](docs/imagod-configuration.md)
- [Documentation Home](docs/README.md)

## Source Of Truth (Code)

- Build and manifest normalization:
  - [`crates/imago-cli/src/commands/build/mod.rs`](crates/imago-cli/src/commands/build/mod.rs)
  - [`crates/imago-cli/src/commands/build/validation.rs`](crates/imago-cli/src/commands/build/validation.rs)
- Dependency and lock resolution:
  - [`crates/imago-cli/src/commands/update/mod.rs`](crates/imago-cli/src/commands/update/mod.rs)
  - [`crates/imago-lockfile/src/lib.rs`](crates/imago-lockfile/src/lib.rs)
- Wire contracts and validation:
  - [`crates/imago-protocol/src/lib.rs`](crates/imago-protocol/src/lib.rs)
  - [`crates/imago-protocol/src/messages`](crates/imago-protocol/src/messages)
- Daemon runtime boundary:
  - [`crates/imagod-server/src/protocol_handler.rs`](crates/imagod-server/src/protocol_handler.rs)
  - [`crates/imagod-control/src/orchestrator.rs`](crates/imagod-control/src/orchestrator.rs)
  - [`crates/imagod-control/src/service_supervisor.rs`](crates/imagod-control/src/service_supervisor.rs)

For generated API docs:

```bash
cargo doc --workspace --no-deps
```

## Development Checks

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

Apache-2.0
