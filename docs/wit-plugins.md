# WIT Plugins

Imago supports plugin dependencies through WIT sources and, for Wasm plugins, optional component sources.

## Plugin Kinds

- `native`: linked into the runtime process.
- `wasm`: provided as component artifacts and loaded by runtime.

## Dependency Declaration

```toml
[[dependencies]]
name = "imago:admin"
version = "0.1.0"
kind = "native"
wit = "file://../../plugins/imago-admin/wit"

[capabilities.deps]
"imago:admin" = ["*"]
```

```toml
[[dependencies]]
name = "example:plugin"
version = "1.2.3"
kind = "wasm"
wit = "warg://example:plugin@1.2.3"
```

## Source Schemes

Supported WIT source schemes:

- `file://`
- `warg://`
- `oci://`

`imago deps sync` resolves sources into project cache and lock data. `imago artifact build` and `imago service deploy` consume the resolved lock/cache state instead of resolving from remote paths during execution.

## Resolution and Locking

- Resolved WIT data is materialized under project cache paths.
- `wit/deps` is regenerated from lock/cache state.
- `imago.lock` is the source of truth for resolved digests and transitive package records.
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
