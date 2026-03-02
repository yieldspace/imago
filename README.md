# imago

Imago is a Wasm Component deployment and runtime platform for embedded Linux systems.
It focuses on lightweight execution, explicit capability boundaries, and a predictable remote deploy workflow.

Start with the documentation landing page:

- [Documentation Home](docs/README.md)

## Important Notice

imago is under development and currently intended for use on private networks.

## Highlights

- Wasm Component as the deployable unit
- Capability-based permission model
- Consistent deploy/run/stop workflow for local and remote targets
- Embedded Linux oriented runtime footprint

## Quickstart

See [QUICKSTART.md](QUICKSTART.md) for step-by-step setup and local example execution.

## imagod Installer (Release Asset Based)

`imagod` can be installed from GitHub Release assets with environment-aware target resolution:

```bash
./scripts/install_imagod.sh
```

Useful options:

- `--tag <semver|imagod-vX.Y.Z>`
- `--libc <auto|gnu|musl>`
- `--repo <owner/repo>`
- `--install-dir <path>`
- `--no-service`
- `--dry-run`

Defaults:

- Source: GitHub Release assets (`imagod-<target-triple>` and `imagod-<target-triple>.sha256`)
- Linux only
- Install path: `/usr/local/bin` (root) or `~/.local/bin` (non-root)
- Service setup: `systemd` first, then `init.d`, otherwise binary-only

`GH_TOKEN` is optional for public repositories and required when accessing private releases.

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
  - [`crates/imago-cli/src/lockfile/mod.rs`](crates/imago-cli/src/lockfile/mod.rs)
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
