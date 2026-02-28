# WIT Plugins

Imago supports plugin dependencies through WIT sources and, for Wasm plugins, optional component sources.

## Plugin Kinds

- `native`: linked into the runtime process.
- `wasm`: provided as component artifacts and loaded by runtime.

## Dependency Declaration

```toml
[[dependencies]]
version = "0.1.0"
kind = "native"
path = "../../plugins/imago-admin/wit"

[capabilities.deps]
"imago:admin" = ["*"]
```

```toml
[[dependencies]]
version = "1.2.3"
kind = "wasm"
wit = "example:plugin"
registry = "wa.dev"
```

## Source Keys

Each dependency/binding uses exactly one source key:

- `wit = "<namespace>:<name...>"` (WARG resolution)
- `oci = "<registry>/<namespace>/<name...>"` (OCI resolution)
- `path = "<local-path|file://...|http(s)://...>"`

Rules:

- `wit` / `oci` values must not include URL schemes.
- Source values must not include `@version`; use sibling `version`.
- `sha256` is optional and verified when provided.

`imago deps sync` resolves sources into project cache and lock data. `imago artifact build` and `imago service deploy` consume the resolved lock/cache state instead of resolving from remote paths during execution.

## Resolution and Locking

- Resolved WIT data is materialized under project cache paths.
- `wit/deps` is regenerated from lock/cache state.
- Component-decoded `root:component` metadata is used for validation/materialization planning, but `root:component` itself is not written to `wit/deps`.
- For component-sourced dependencies, non-`wasi:*` refs discovered from component world `import`/`export`/`include` must be present in `[[dependencies]]` with exact resolved package `name+version`.
- `imago deps sync` inspects top-level `wit/*.wit` files and merges discovered `wasi:*@<version>` refs with component-world discovered `wasi:*@<version>` refs.
- Merged `wasi:*` refs are resolved with `registry = "wasi.dev"` and materialized into `wit/deps`; version conflicts fail closed.
- `imago.lock` is the source of truth for resolved digests and transitive package records.
- `imago.lock.wit_packages[*].versions[*].via` may be empty (`[]`) for auto-hydrated records.
- `resolved_at` is removed from lock dependency/binding entries; older lockfiles with that field are rejected.
- Missing or mismatched cache/lock data requires running `imago deps sync`.

## Native Plugin WIT Publishing

For plugin crates under `plugins/*` with `wit/package.wit`:

- Keep `wkg.lock` committed and updated.
- Validate lock consistency in CI.
- Publish tags in `<plugin-dir>@<version>` format.
- Keep the tag version aligned with the WIT package version.

## Source References

- Dependency resolution and lock output:
  - [`crates/imago-cli/src/commands/update/mod.rs`](../crates/imago-cli/src/commands/update/mod.rs)
  - [`crates/imago-lockfile/src/lib.rs`](../crates/imago-lockfile/src/lib.rs)
  - [`crates/imago-lockfile/src/resolve.rs`](../crates/imago-lockfile/src/resolve.rs)
- Build-time dependency normalization:
  - [`crates/imago-cli/src/commands/build/mod.rs`](../crates/imago-cli/src/commands/build/mod.rs)
- Runtime dependency execution boundary:
  - [`crates/imagod-control/src/orchestrator.rs`](../crates/imagod-control/src/orchestrator.rs)
