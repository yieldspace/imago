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
- Direct dependency outputs and transitive outputs in `wit/deps` use versioned directory names (for example `wit/deps/wasi-cli-0.2.6`).
- `root:component` dependencies are written to `wit/deps` as export-only WIT (world imports are removed and only exported interfaces are emitted).
- For component-sourced dependencies, non-`wasi:*` refs discovered from component world `import`/`export`/`include` must be present in `[[dependencies]]` with exact resolved package `name+version`.
- `imago deps sync` recursively tracks foreign package refs from component-world refs and from the `wit/` package dir closure.
- Auto-fetch is only for `wasi:*`; these packages are always resolved from `wasi.dev` and materialized into `wit/deps`.
- Non-`wasi` refs are never auto-fetched; they must already exist in declared dependencies with matching `name+version`.
- Version conflicts for the same package fail closed.
- For `path = "http(s)://..."` sources that fall back to plain `.wit`, diagnostics keep the HTTP origin (`http source '...'`) and do not rewrite it to Warg placeholder metadata.
- `imago.lock` has two sections: `[requested]` and `[resolved]`.
- `[requested]` records normalized request identities and `fingerprint`.
- `[resolved]` records resolved dependency/binding entries and transitive package graph (`packages` + `package_edges`).
- `requested.dependencies[].id` is derived from normalized request identity (`kind/version/source/component/declared_requires/capabilities`).
- `resolved.packages[].package_ref` must match canonical `<name>@<version-or-*>#<registry-or-empty>`.
- `resolved.package_edges[].reason` accepts only:
  - `declared-requires`
  - `wit-import`
  - `component-world`
  - `auto-wasi`
  - `wit-dir-closure`
- Build/deploy recompute `[requested].fingerprint` from `imago.toml` and require exact match.
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
