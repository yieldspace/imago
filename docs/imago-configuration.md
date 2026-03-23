# imago.toml reference

This page is a practical reference for `imago.toml`.
Source of truth is the codebase (module docs, type definitions, validation logic, and tests).

## JSON Schema

You can enable editor completion/validation by adding this root key:

```toml
"$schema" = "https://raw.githubusercontent.com/yieldspace/imago/main/schemas/imago.schema.json"
```

## Sections

- [The top-level fields section](#the-top-level-fields-section)
- [The `[build]` section](#the-build-section)
- [The `[target.<name>]` section](#the-targetname-section)
- [The `[[assets]]` section](#the-assets-section)
- [The `[http]` section](#the-http-section)
- [The `[socket]` section](#the-socket-section)
- [The `[resources]` section](#the-resources-section)
- [The `[capabilities]` section](#the-capabilities-section)
- [The `[[bindings]]` section](#the-bindings-section)
- [The `[[dependencies]]` section](#the-dependencies-section)
- [The `[namespace_registries]` section](#the-namespace_registries-section)
- [Legacy sections](#legacy-sections)

<a id="the-top-level-fields-section"></a>
## The top-level fields section

This section defines service identity and execution model.
These keys are read from the root TOML table and are not nested under a `[package]` table.

### The `name` field

- Type: `string` or `table`
- Required/Optional: Required.
- Accepted values / Constraints:
  - Literal string: 1..=63 characters; ASCII `[A-Za-z0-9._-]` only; must not contain `/`, `\\`, or `..`.
  - Resolver table: enable exactly one of `cargo = true` or `pyproject = true`.
- Default: none.
- Example:

```toml
name = "example-service"
```

```toml
name.cargo = true
```

```toml
name.pyproject = true
```

- Resolution notes:
  - `name.cargo = true` reads `Cargo.toml` `[package].name` from the same project root as `imago.toml`.
  - `name.pyproject = true` reads `pyproject.toml` `[project].name` from the same project root as `imago.toml`.
  - Resolver lookup is root-only and fail-closed. It does not search parent directories.
- Validation error notes: missing, empty, invalid characters, missing sibling files, missing source keys, or invalid resolver tables cause `imago artifact build` to fail.

### The `main` field

- Type: `string` (relative filesystem path)
- Required/Optional: Required.
- Accepted values / Constraints: non-empty; relative path only; no Windows drive prefix; no backslashes; `.` segments are normalized away; `..` parent segments are allowed; file must exist.
- Default: none.
- Example:

```toml
main = "build/app.wasm"
```

- Validation error notes: missing file or unsafe path syntax causes `imago artifact build` to fail. The build output rewrites `main` to a hashed artifact filename in `build/manifest.json`, so release manifests still use a traversal-free entry name.

### The `type` field

- Type: `string` enum
- Required/Optional: Required.
- Accepted values / Constraints: one of `cli`, `http`, `socket`, `rpc`.
- Default: none.
- Example:

```toml
type = "http"
```

- Validation error notes: unknown values fail validation; `http` requires `[http]`; `socket` requires `[socket]`; `http` and `socket` sections are rejected for other types.

### The `restart` field

- Type: `string` enum
- Required/Optional: Optional.
- Accepted values / Constraints: one of `never`, `on-failure`, `always`, `unless-stopped`.
- Default: `never`.
- Example:

```toml
restart = "always"
```

- Validation error notes: any other value fails validation.

<a id="the-build-section"></a>
## The [build] section

This section configures the local build command used by `imago artifact build`.

### The `command` field

- Type: `string` or `array(string)`
- Required/Optional: Optional.
- Accepted values / Constraints: a string must be non-empty; an array must be non-empty and contain only strings.
- Default: none (no command execution; only `main` file checks run).
- Example:

```toml
[build]
command = "cargo component build --release"
```

- Validation error notes: wrong type or empty command fails validation.

### The `wit_world` field

- Type: `string`
- Required/Optional: Optional.
- Accepted values / Constraints: non-empty world name from the local `wit/` package.
- Default: none. When omitted, dependency capability validation calls `select_world(..., None)` and inherits its ambiguity error for multi-world packages.
- Example:

```toml
[build]
wit_world = "plugin-imports"
```

- Validation error notes: empty values fail validation. Unknown world names fail during `imago artifact build` when project WIT imports are inspected.

<a id="the-targetname-section"></a>
## The [target.<name>] section

This section configures remote deployment targets.

### The `remote` field

- Type: `string`
- Required/Optional: Required for the selected target.
- Accepted values / Constraints:
  - SSH target only: `ssh://[user@]host[:port][?socket=/absolute/path/to/imagod.sock]`
  - `user@` is optional. When omitted, the system `ssh` command uses its default user resolution.
  - SSH query parameters are validated strictly; only `socket=` is accepted.
  - SSH targets must not include a password, path, or fragment.
- Default: none.
- Example:

```toml
[target.default]
remote = "ssh://localhost?socket=/run/imago/imagod.sock"
```

```toml
[target.edge]
remote = "ssh://root@edge-box?socket=/run/imago/imagod.sock"
```

- Validation error notes: missing selected target, non-SSH endpoints such as `host:port`, invalid SSH URIs, or unsupported SSH query parameters cause validation/deploy failure.

When an SSH target uses `?socket=...`, that path must match the target daemon's `control_socket_path`.
Loopback targets without `user@` or `:port` (`ssh://localhost?...`, `ssh://127.0.0.1?...`, `ssh://[::1]?...`) connect directly to that local control socket.
Other targets still use SSH plus `imagod proxy-stdio`.
Host verification and authentication are delegated to OpenSSH; `~/.imago/known_hosts` is not used.
Node-to-node RPC authorities remain separate from target remotes and are written as `rpc://host:port` in `trust cert upload` / `trust cert replicate`.

<a id="the-assets-section"></a>
## The [[assets]] section

This section declares files bundled as service assets.

### The `path` field

- Type: `string` (relative file path)
- Required/Optional: Required in each `[[assets]]` entry.
- Accepted values / Constraints: non-empty; same safe-path rules as `main`; file must exist.
- Default: none.
- Example:

```toml
[[assets]]
path = "assets/config.json"
mode = "0644"
```

- Validation error notes: missing/unsafe path fails validation. Additional keys are accepted and forwarded to `manifest.assets[]` as JSON values.
- Usage note: pair `[[assets]]` with `[[resources.read_only_mounts]]` when guest code needs to open bundled files directly, for example `wasi:nn` model files under `/app/assets`.

<a id="the-http-section"></a>
## The [http] section

This section is valid only when `type = "http"`.

### The `port` field

- Type: `integer`
- Required/Optional: Required when `type = "http"`.
- Accepted values / Constraints: `1..=65535`.
- Default: none.
- Example:

```toml
[http]
port = 8080
```

- Validation error notes: missing or out-of-range values fail validation.

### The `max_body_bytes` field

- Type: `integer`
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=33554432` (32 MiB).
- Default: `4194304` (4 MiB).
- Example:

```toml
[http]
port = 8080
max_body_bytes = 4194304
```

- Validation error notes: out-of-range values fail validation.

<a id="the-socket-section"></a>
## The [socket] section

This section is valid only when `type = "socket"`.

### The `protocol` field

- Type: `string` enum
- Required/Optional: Required when `type = "socket"`.
- Accepted values / Constraints: one of `udp`, `tcp`, `both`.
- Default: none.
- Example:

```toml
[socket]
protocol = "tcp"
```

- Validation error notes: unknown values fail validation.

### The `direction` field

- Type: `string` enum
- Required/Optional: Required when `type = "socket"`.
- Accepted values / Constraints: one of `inbound`, `outbound`, `both`.
- Default: none.
- Example:

```toml
[socket]
direction = "inbound"
```

- Validation error notes: unknown values fail validation.

### The `listen_addr` field

- Type: `string`
- Required/Optional: Required when `type = "socket"`.
- Accepted values / Constraints: valid IP address literal.
- Default: none.
- Example:

```toml
[socket]
listen_addr = "0.0.0.0"
```

- Validation error notes: non-IP values fail validation.

### The `listen_port` field

- Type: `integer`
- Required/Optional: Required when `type = "socket"`.
- Accepted values / Constraints: `1..=65535`.
- Default: none.
- Example:

```toml
[socket]
listen_port = 9000
```

- Validation error notes: out-of-range values fail validation.

<a id="the-resources-section"></a>
## The [resources] section

This section defines runtime resource policy and custom resource metadata.

### The `args` field

- Type: `array(string)`
- Required/Optional: Optional.
- Accepted values / Constraints: each item must be a non-empty string.
- Default: empty array.
- Example:

```toml
[resources]
args = ["--serve"]
```

- Validation error notes: non-string entries fail validation.

### The `http_outbound` field

- Type: `array(string)`
- Required/Optional: Optional.
- Accepted values / Constraints: each rule must be `hostname`, `host:port`, or CIDR; wildcard patterns such as `*` or `*.example.com` are rejected; CIDR rules apply only when request host is an IP literal.
- Default: empty array (manager injects `localhost`, `127.0.0.1`, `::1` at runtime).
- Example:

```toml
[resources]
http_outbound = ["localhost", "api.example.com:443", "10.0.0.0/8"]
```

- Validation error notes: invalid host/port/CIDR or wildcard values fail validation.

### The `env.<KEY>` field

- Type: `string` value in a table
- Required/Optional: Optional.
- Accepted values / Constraints: both key and value must be strings.
- Default: empty table.
- Example:

```toml
[resources.env]
WASI_ONLY = "1"
```

- Validation error notes: non-string key/value types fail validation. `.env` values override duplicate keys.

### The `mounts[].asset_dir` field

- Type: `string`
- Required/Optional: Required in each `[[resources.mounts]]` entry.
- Accepted values / Constraints: must refer to a directory derived from `assets[].path` parent directories; unique across read-write and read-only mount arrays.
- Default: none.
- Example:

```toml
[[resources.mounts]]
asset_dir = "assets"
guest_path = "/app/assets"
```

- Validation error notes: unknown or duplicate `asset_dir` fails validation.

### The `mounts[].guest_path` field

- Type: `string`
- Required/Optional: Required in each `[[resources.mounts]]` entry.
- Accepted values / Constraints: absolute Unix-style path; no path traversal; unique across read-write and read-only mount arrays.
- Default: none.
- Example:

```toml
[[resources.mounts]]
guest_path = "/app/assets"
```

- Validation error notes: non-absolute or duplicate guest path fails validation.

### The `read_only_mounts[].asset_dir` field

- Type: `string`
- Required/Optional: Required in each `[[resources.read_only_mounts]]` entry.
- Accepted values / Constraints: same rules as `mounts[].asset_dir`; unique across both mount arrays.
- Default: none.
- Example:

```toml
[[resources.read_only_mounts]]
asset_dir = "readonly"
guest_path = "/app/readonly"
```

- Validation error notes: unknown or duplicate `asset_dir` fails validation.

### The `read_only_mounts[].guest_path` field

- Type: `string`
- Required/Optional: Required in each `[[resources.read_only_mounts]]` entry.
- Accepted values / Constraints: same rules as `mounts[].guest_path`; unique across both mount arrays.
- Default: none.
- Example:

```toml
[[resources.read_only_mounts]]
guest_path = "/app/readonly"
```

- Validation error notes: non-absolute or duplicate guest path fails validation.
- Usage note: this is the recommended way to expose bundled model files to guest code that calls `wasi:nn`; the runtime does not preload models from `assets`.

### The `usb.paths` field

- Type: `array(string)`
- Required/Optional: Required when using `imago:usb` native plugin.
- Accepted values / Constraints: absolute paths only; empty strings and NUL are rejected; duplicate entries after path normalization are rejected.
- Default: none (missing field fails validation).
- Example:

```toml
[resources.usb]
paths = ["/dev/bus/usb/001/001", "/dev/bus/usb/001/002"]
```

- Validation error notes: missing field, wrong type, non-absolute paths, or normalized duplicates fail runtime startup validation.
- Usage note: when a service uses `imago:camera`, this allowlist must include the actual UVC device path that the camera plugin should open.

### The `usb.max_transfer_bytes` field

- Type: `integer`
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=8388608`.
- Default: `1048576`.
- Example:

```toml
[resources.usb]
max_transfer_bytes = 1048576
```

- Validation error notes: out-of-range values fail runtime startup validation.

### The `usb.max_timeout_ms` field

- Type: `integer`
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=120000`.
- Default: `30000`.
- Example:

```toml
[resources.usb]
max_timeout_ms = 30000
```

- Validation error notes: out-of-range values fail runtime startup validation.

### The `usb.max_paths` field

- Type: `integer`
- Required/Optional: Optional.
- Accepted values / Constraints: `0..=256`; `usb.paths` entry count must not exceed this value.
- Default: `128`.
- Example:

```toml
[resources.usb]
max_paths = 128
```

- Validation error notes: out-of-range values or `paths` overflow fail runtime startup validation.

### The `usb.bulk_ring_chunk_bytes` field

- Type: `integer`
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=usb.max_transfer_bytes`.
- Default: `min(16384, usb.max_transfer_bytes)`.
- Example:

```toml
[resources.usb]
bulk_ring_chunk_bytes = 16384
```

- Validation error notes: zero, negative, non-integer, or values above `usb.max_transfer_bytes` fail runtime startup validation.

### The `usb.bulk_ring_slots` field

- Type: `integer`
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=256`; `usb.bulk_ring_chunk_bytes * usb.bulk_ring_slots` must be `<= 67108864`.
- Default: `16`.
- Example:

```toml
[resources.usb]
bulk_ring_slots = 16
```

- Validation error notes: zero, negative, non-integer, out-of-range values, or capacity products above `67108864` fail runtime startup validation.

### USB runtime behavior (`imago:usb`)

- `provider.list-openable-paths` returns the normalized allowlist defined by `resources.usb.paths`.
- `provider.list-openable-devices` returns currently connected devices whose Linux path is allowlisted.
- `provider.poll-device-event(timeout-ms)` returns only allowlisted connection events (`pending`, `connected`, `disconnected`); non-allowlisted events are dropped internally.
- `provider.open-device(path)` rejects non-allowlisted paths before backend open attempts.
- Per-device worker channels are bounded; saturated workers can return `usb-error.busy`.
- All transfer APIs (`control`, `bulk`, `interrupt`, `isochronous`) enforce `usb.max_transfer_bytes` and `usb.max_timeout_ms`.
- `claimed-interface.isochronous-in(endpoint, length, packets, timeout-ms)` returns each packet descriptor's actual payload bytes concatenated in packet order.
- `claimed-interface.bulk-read(endpoint, timeout-ms)` reads one chunk from a per-endpoint bounded ring buffer (size from `usb.bulk_ring_chunk_bytes`).
- Ring overflow uses drop-oldest backpressure and is observable via `claimed-interface.bulk-read-stats(endpoint)`.

### The `<custom_key>` field

- Type: any TOML value (`boolean`, `string`, `integer`, `float`, `array`, `table`, datetime)
- Required/Optional: Optional.
- Accepted values / Constraints: key must be non-empty.
- Default: none.
- Example:

```toml
[resources]
feature_enabled = true
allowed_devices = ["/dev/i2c-1", "/dev/i2c-2"]

[resources.policy]
mode = "strict"

[resources.gpio]
digital_pins = [
  { label = "A27", aliases = ["blue-led", "status-led"] },
]

[resources.usb]
paths = ["/dev/bus/usb/001/001"]
max_transfer_bytes = 1048576
bulk_ring_chunk_bytes = 16384
bulk_ring_slots = 16
```

- `[[dependencies]] kind = "wasm"` entries that embed an `imago.resources.v1` custom section are treated as resource providers automatically during `imago deps sync` / build. Their `resources` payload is merged first, and service-side `resources` is applied afterward using provider merge policies.
- `resources.gpio.digital_pins` works in two modes. Without a provider it is an inline catalog and each entry must supply the full runtime fields (`label`, `value_path`, `supports_input`, `supports_output`, `default_active_level`, `allow_pull_resistor`). With a provider present it becomes a keyed patch surface over existing labels, so patch entries may override fields such as `aliases`, but unknown labels fail validation.
- `resources.gpio.profile.path` remains available as a local TOML profile source. Only `path` is supported. If it would populate the same canonical path as an embedded provider (currently `resources.gpio.digital_pins`), build fails instead of applying implicit precedence.
- Provider merge policies default to `sealed`. `mergeable` allows service overrides under the declared path, and `required` also demands that the service override that path. `resources.gpio.digital_pins` is the only keyed array merge in v1; other provider-owned arrays are treated atomically.
- Validation error notes: empty keys fail validation. `resources.usb` rejects invalid path/limit settings during runtime startup.

<a id="the-capabilities-section"></a>
## The [capabilities] section

This section defines explicit allow rules for dependency and WASI calls.

### The `privileged` field

- Type: `boolean`
- Required/Optional: Optional.
- Accepted values / Constraints: `true` or `false`.
- Default: `false`.
- Example:

```toml
[capabilities]
privileged = false
```

- Validation error notes: non-boolean values fail validation. `true` bypasses capability tables.

### The `deps` field

- Type: `"*"` string or `table(map<string, array(string)>)`
- Required/Optional: Optional.
- Accepted values / Constraints: string form must be exactly `"*"`; table keys are dependency package names or `"*"`; values are arrays of non-empty function names or `"*"`.
- Default: empty table.
- Example:

```toml
[capabilities]
deps = "*"
```

- Validation error notes: non-wildcard strings or wrong table value types fail validation.

### The `wasi` field

- Type: `boolean` or `table(map<string, array(string)>)`
- Required/Optional: Optional.
- Accepted values / Constraints: `true` means wildcard allow; `false` means empty policy; table form maps WASI interface names to allowed function arrays.
- Default: empty table.
- Example:

```toml
[capabilities]
wasi = true
```

- Validation error notes: wrong value types fail validation.
- Usage note: this allowlist also governs imports such as `wasi:nn/*`; guest-side `wasi-nn` calls require the corresponding WASI capability rules.
- Runtime note: when `imagod` is built with `wasi-nn-cvitek`, `wasi:nn/graph.load` accepts only precompiled `.cvimodel` bytes via `graph-encoding = autodetect` and `execution-target = tpu`. ONNX or PyTorch models must be converted outside `imago` before deploy. Release assets for such variants are published as `imagod-<target>+wasi-nn-cvitek`, and the build falls back to auto-downloading the pinned SG200x TPU SDK when `IMAGO_CVITEK_SDK_ROOT` / `CVI_TPU_SDK_ROOT` are unset. Linux `riscv64` `musl` builds prefer static linking, but automatically fall back to `IMAGO_CVITEK_LINK_MODE=dynamic` when `riscv64-unknown-linux-musl-g++` is unavailable; in that mode `imagod` looks for CVITEK TPU shared libraries in the system loader path or next to the binary under `lib/`.

<a id="the-bindings-section"></a>
## The [[bindings]] section

This section defines allowed service-to-service interface bindings.

### Required fields

- `name` (`string`): service name (same validation as top-level `name`).
- `version` (`string`): required; source text must not embed `@version`.
- Exactly one source key: `wit`, `oci`, or `path`.

### Source keys

- `wit` (`string`): plain package name only (example: `acme:rpc-api`).
  - `warg://` prefix is not allowed.
  - `@version` is not allowed.
- `oci` (`string`): `<registry>/<namespace>/<name...>` (example: `ghcr.io/acme/plugins/rpc-api`).
  - `oci://` prefix is not allowed.
  - `@version` is not allowed.
- `path` (`string`): local path / `file://...` path / `http(s)://...` URL.
- `registry` (`string`, optional): allowed only when `wit` is used.
- `sha256` (`string`, optional): 64 hex chars. If present, `deps sync` verifies source bytes.

### Expansion behavior

- `imago deps sync` resolves each binding source, writes hydrated WIT under `wit/deps`, then expands interfaces into `manifest.bindings[]` as `{ "name": "<service>", "wit": "<package>/<interface>" }`.
- If omitted, `manifest.bindings=[]` (deny-by-default).

<a id="the-dependencies-section"></a>
## The [[dependencies]] section

This section defines plugin dependencies and their resolution sources.

### Required fields

- `version` (`string`, required)
- `kind` (`native` or `wasm`, required)
- Exactly one source key: `wit`, `oci`, or `path` (required)

`dependencies[].name` is removed. Dependency identity is the resolved top-level package name.

### Source keys

- `wit` (`string`): plain package name only (example: `acme:plugin/example`).
  - `warg://` prefix is not allowed.
  - `@version` is not allowed.
- `oci` (`string`): `<registry>/<namespace>/<name...>` (example: `ghcr.io/acme/plugins/example`).
  - `oci://` prefix is not allowed.
  - `@version` is not allowed.
- `path` (`string`): local path / `file://...` path / `http(s)://...` URL.
- `registry` (`string`, optional): allowed only with `wit`.
- `sha256` (`string`, optional): 64 hex chars. If present, `deps sync` verifies source bytes.

### Additional fields

- `requires` (`array(string)`, optional): resolved package names.
- `[dependencies.capabilities]` (optional): same shape as top-level `[capabilities]`.

### `[dependencies.component]` (optional, `kind="wasm"` only)

- Source keys: exactly one of `wit`, `oci`, or `path`.
- `registry` is allowed only with `component.wit`.
- `sha256` is optional (64 hex chars) and verified when provided.
- If the dependency `wit` / `oci` source already resolves to a Wasm component release, this section can be omitted and `imago deps sync` derives the component from that same source.

### Published Wasm camera plugin example

The published `imago:camera@0.1.0` plugin can be consumed directly from GHCR without an explicit `[dependencies.component]` block. The `imago:usb@0.3.0` dependency remains native:

```toml
[[dependencies]]
version = "0.3.0"
kind = "native"
oci = "ghcr.io/yieldspace/imago/usb"

[[dependencies]]
version = "0.1.0"
kind = "wasm"
oci = "ghcr.io/yieldspace/imago/camera"
requires = ["imago:usb"]

[dependencies.capabilities.deps]
"imago:usb" = ["*"]
```

### Local Wasm camera plugin example

The `imago:camera@0.1.0` plugin is a Wasm dependency that imports `imago:usb@0.3.0` as a native dependency. The app manifest needs both entries, plus a component source path for the camera plugin artifact:

```toml
[[dependencies]]
version = "0.3.0"
kind = "native"
path = "../../plugins/imago-usb/wit"

[[dependencies]]
version = "0.1.0"
kind = "wasm"
path = "../../plugins/imago-camera/wit"
requires = ["imago:usb"]

[dependencies.component]
path = "../../target/wasm32-wasip2/release/imago_plugin_imago_camera.wasm"

[dependencies.capabilities.deps]
"imago:usb" = ["*"]
```

The service itself still needs `[capabilities.deps] "imago:camera" = ["*"]` to call the camera API. A typical local flow is:

```bash
cd examples/local-imagod-plugin-camera
cargo build --manifest-path ../../Cargo.toml -p imago-plugin-imago-camera --target wasm32-wasip2 --release
cargo run --manifest-path ../../Cargo.toml -p imago-cli -- deps sync
cargo run --manifest-path ../../Cargo.toml -p imagod -- --config "$(pwd)/imagod.toml"
```

After that, `cargo build --target wasm32-wasip2 --release` in the example directory builds the app, and `imago-cli service deploy --target default --detach` deploys it against the local `imagod` instance.

### Resolution and lock behavior

- `imago deps sync` resolves dependencies and writes cache under `.imago/deps`.
- `wit/deps` is regenerated on each sync.
- `wit/deps` directory names include package version when available (for example `wit/deps/wasi-io-0.2.6`).
- `root:component` dependencies are emitted as export-only WIT (imports removed; exported interfaces only).
- Component world `non-wasi` refs must match declared dependencies by resolved package name and version.
- `deps sync` recursively tracks foreign package refs from component world refs and the `wit/` package dir closure.
- `wasi:*` refs are resolved from `wasi.dev` and materialized into `wit/deps`.
- Non-`wasi` refs are not auto-fetched; declared dependencies must provide matching `name+version`.
- `imago.lock` is split into `[requested]` and `[resolved]`.
- `[requested]` stores normalized dependency/binding requests and a `fingerprint`.
- `[resolved]` stores `dependencies`, `bindings`, `packages`, and `package_edges`.
- `requested.dependencies[].id` is derived from full normalized request identity (`kind/version/source_kind/source/registry/sha256/component_source_kind/component_source/component_registry/component_sha256/declared_requires/capabilities`).
- `resolved.packages[].package_ref` must equal canonical `<name>@<version-or-*>#<registry-or-empty>`.
- `resolved.package_edges[].reason` must be one of:
  - `declared-requires`
  - `wit-import`
  - `component-world`
  - `auto-wasi`
  - `wit-dir-closure`
- Build/deploy recompute `[requested].fingerprint` from `imago.toml` and fail closed on mismatch.

<a id="the-namespace_registries-section"></a>
## The [namespace_registries] section

This section overrides WARG registry hosts by namespace.

### The `<namespace>` field

- Type: `string` value in a table
- Required/Optional: Optional.
- Accepted values / Constraints: table key is namespace; value is registry host string.
- Default: none.
- Example:

```toml
[namespace_registries]
wasi = "wasi.dev"
```

- Validation error notes: values must be strings. This table applies only when a `wit = "<namespace>:<pkg>"` source omits its registry.

## Related source modules

- Build and manifest normalization:
  - [`crates/imago-cli/src/commands/build/mod.rs`](../crates/imago-cli/src/commands/build/mod.rs)
- Config validation helpers:
  - [`crates/imago-cli/src/commands/build/validation.rs`](../crates/imago-cli/src/commands/build/validation.rs)
- Dependency and lock resolution:
  - [`crates/imago-cli/src/commands/update/mod.rs`](../crates/imago-cli/src/commands/update/mod.rs)
  - [`crates/imago-cli/src/lockfile/mod.rs`](../crates/imago-cli/src/lockfile/mod.rs)
