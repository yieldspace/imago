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
imago init .

# Non-interactive (--json/CI/no TTY)
imago init services/example --lang rust
imago --json init services/example --lang generic
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
cargo run -p imago-cli -- deploy --target default --detach
cargo run -p imago-cli -- logs local-imagod-app --tail 200
```

## CLI Output Modes

Mode priority:

1. `--json`
2. `CI=true`
3. Rich terminal output

Example success summary (`--json`):

```json
{"type":"command.summary","command":"deploy","status":"completed","duration_ms":1234,"timestamp":"2026-02-20T12:34:56Z","meta":{},"error":null}
```

Example log line (`logs --json`):

```json
{"type":"log.line","name":"local-imagod-app","stream":"stdout","timestamp":"1739982001","log":"local-imagod-app started"}
```

Example failure line:

```json
{"type":"command.error","command":"logs","message":"...","stage":"logs","code":"E_UNKNOWN"}
```

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
