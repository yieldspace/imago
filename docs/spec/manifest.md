# Manifest Specification (`build/manifest.json`)

## Purpose

`build/manifest.json` is the normalized deployment contract produced by `imago build`.
`imagod` and runner components MUST interpret this format consistently.

## Required Fields

- `name`: service name.
- `main`: component path relative to manifest directory.
- `type`: `cli`, `http`, `socket`, or `rpc`.
- `target`: resolved target map.
- `assets`: bundled asset list.
- `bindings`: service invocation authorization list.
- `dependencies`: plugin dependency records.
- `capabilities`: normalized capability policy.
- `hash`: integrity metadata.

## Optional Mode-Specific Fields

- `http` for `type=http`.
- `socket` for `type=socket`.
- `wasi` when arguments/env/network/mount rules are present.

## Hash Contract

`hash` MUST include:

- `algorithm = "sha256"`
- `value`
- `targets` containing `wasm`, `manifest`, and `assets`

Digest input order MUST be:

1. Component bytes (`main` target)
2. Normalized manifest bytes (with computed hash value)
3. Asset bytes sorted by path

## Bindings Contract

Each binding entry MUST include:

- `name`: target service
- `wit`: `<package>/<interface>`

Bindings are expanded from resolved WIT sources during update/build steps.

## Dependencies Contract

Each dependency entry includes:

- `name`, `version`, `kind`, `wit`
- optional `requires`
- optional `component` for `kind=wasm`
- optional `capabilities`

For `kind=wasm`, `component.path` and `component.sha256` MUST be present in build output.

## Capability Contract

- `privileged=true` means full allow.
- otherwise permissions are explicit through `deps` and `wasi`.
- unspecified permissions MUST be denied.

## WASI Contract

When present, `wasi` MAY include:

- `args`: string array
- `env`: string map
- `http_outbound`: allowlist entries
- `mounts` and `read_only_mounts`

Mount entries MUST use `asset_dir` + absolute `guest_path`.

## Validation Requirements

`imagod` MUST reject manifests with:

- missing required fields
- unknown enum values
- invalid mode-specific field combinations
- invalid hash metadata
- malformed bindings/dependencies/capabilities/wasi structures

## Examples

- Valid manifest: [examples/manifest.valid.json](./examples/manifest.valid.json)
- Invalid missing required fields: [examples/manifest.invalid.missing-required.json](./examples/manifest.invalid.missing-required.json)
- Invalid type value: [examples/manifest.invalid.bad-type.json](./examples/manifest.invalid.bad-type.json)
- Invalid hash mismatch: [examples/manifest.invalid.hash-mismatch.json](./examples/manifest.invalid.hash-mismatch.json)
- Invalid `wasi.env` shape: [examples/manifest.invalid.wasi-env-shape.json](./examples/manifest.invalid.wasi-env-shape.json)
