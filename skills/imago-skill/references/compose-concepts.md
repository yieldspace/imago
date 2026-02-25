# Compose Concepts

## Purpose

Use `imago-compose.toml` to orchestrate multiple service projects that each have their own `imago.toml`.
Use this document for multi-service topology and compose-only constraints.
For single-service behavior, read `imago-core-concepts.md`.

## Model Overview

- `imago.toml`: single-service definition (build inputs, runtime, target defaults, capabilities).
- `imago-compose.toml`: multi-service composition layer.
- Compose execution path:
  1. Resolve `profile`.
  2. Resolve compose `config` via `profile.<name>.config`.
  3. Iterate `compose.<config>.services[*].imago`.
  4. Resolve target from `target.<name>`.
  5. Execute compose command (`build`, `update`, `deploy`, `logs`, `ps`).

## Minimal `imago-compose.toml`

```toml
[[compose.devstack.services]]
imago = "services/rpc-greeter/imago.toml"

[[compose.devstack.services]]
imago = "services/cli-client/imago.toml"

[profile.dev]
config = "devstack"

[target.default]
remote = "127.0.0.1:4443"
server_name = "localhost"
client_key = "certs/client.key"
```

## Key Constraints

- `imago-compose.toml` must be readable from the current project root.
- `profile.<name>.config` must be non-empty and must point to an existing `compose.<config>` section.
- `compose.<config>.services` must be non-empty.
- Every `service.imago` value must:
  - be non-empty,
  - point to a file named `imago.toml`,
  - exist on disk.
- `target.<name>.remote` is required and must be non-empty.
- `target.<name>.server_name` and `target.<name>.client_key` are optional but must be non-empty if present.
- `compose ps` requires unique resolved service names; duplicate service names in the same profile fail.

## Targeting Model

- Compose commands accept one `--target` value per invocation.
- For multi-imagod scenarios, run separate commands for each target/profile pair.
- Do not assume one compose command can deploy to multiple targets at once.

## Compose Command Set

- `compose build <profile> --target <target>`
- `compose update <profile>`
- `compose deploy <profile> --target <target>`
- `compose logs <profile> --target <target> [--name <service>] [--follow] [--tail N]`
- `compose ps <profile> --target <target>`

`<profile>` is a profile name defined under `[profile.<name>]` (for example `prepare`, `dev`, `client`, `greeter`), not a compose subcommand.

## Practical Teaching Pattern

1. Explain the map (`profile -> config -> services -> target`).
2. Confirm names from `imago-compose.toml`.
3. Suggest the shortest valid command sequence.
4. If command fails, map stderr to structural constraints first, then runtime causes.
