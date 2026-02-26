# imago.toml reference

This page is a practical reference for `imago.toml`.
Source of truth is the codebase (module docs, type definitions, validation logic, and tests).

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

- Type: `string`
- Required/Optional: Required.
- Accepted values / Constraints: 1..=63 characters; ASCII `[A-Za-z0-9._-]` only; must not contain `/`, `\\`, or `..`.
- Default: none.
- Example:

```toml
name = "example-service"
```

- Validation error notes: missing, empty, or invalid characters cause `imago build` to fail.

### The `main` field

- Type: `string` (relative file path)
- Required/Optional: Required.
- Accepted values / Constraints: non-empty; relative path only; no Windows drive prefix; no backslashes; no path traversal; file must exist.
- Default: none.
- Example:

```toml
main = "build/app.wasm"
```

- Validation error notes: missing file or unsafe path causes `imago build` to fail.

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

- Validation error notes: any other value fails validation; `runtime.restart_policy` is rejected as a legacy key.

<a id="the-build-section"></a>
## The [build] section

This section configures the local build command used by `imago build`.

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

<a id="the-targetname-section"></a>
## The [target.<name>] section

This section configures remote deployment targets.

### The `remote` field

- Type: `string`
- Required/Optional: Required for the selected target.
- Accepted values / Constraints: endpoint string; validated in deploy/run paths.
- Default: none.
- Example:

```toml
[target.default]
remote = "127.0.0.1:4443"
```

- Validation error notes: missing selected target or invalid endpoint causes deploy/run failure.

### The `server_name` field

- Type: `string`
- Required/Optional: Optional.
- Accepted values / Constraints: must be a string; used as authority for known-host matching.
- Default: none.
- Example:

```toml
[target.default]
server_name = "node-a.example.com"
```

- Validation error notes: non-string values fail validation.

### The `client_key` field

- Type: `string` (path)
- Required/Optional: Optional for `imago build`; required for deploy/run/stop/logs/ps paths.
- Accepted values / Constraints: non-empty; no path traversal; no backslashes; no Windows drive prefix; relative paths resolve from project root; absolute paths are allowed.
- Default: none.
- Example:

```toml
[target.default]
client_key = "certs/client.key"
```

- Validation error notes: invalid paths fail validation; missing key in deploy paths fails the command.

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
- Accepted values / Constraints: `1..=67108864` (64 MiB).
- Default: `8388608` (8 MiB).
- Example:

```toml
[http]
port = 8080
max_body_bytes = 8388608
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
  { label = "GPIO17", value_path = "/sys/class/gpio/gpio17/value", supports_input = true, supports_output = true, default_active_level = "active-high", allow_pull_resistor = true }
]
```

- Validation error notes: empty keys fail validation. `resources.gpio.digital_pins` rejects duplicated `label` and duplicated `value_path` during runtime startup.

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

<a id="the-bindings-section"></a>
## The [[bindings]] section

This section defines allowed service-to-service interface bindings.

### The `name` field

- Type: `string`
- Required/Optional: Required in each `[[bindings]]` entry.
- Accepted values / Constraints: service name with the same character rules as top-level `name`.
- Default: none.
- Example:

```toml
[[bindings]]
name = "rpc-provider"
wit = "warg://acme:rpc-api@0.1.0"
```

- Validation error notes: missing or invalid target name fails validation.

### The `wit` field

- Type: `string`
- Required/Optional: Required in each `[[bindings]]` entry.
- Accepted values / Constraints: must use one of `file://`, `warg://`, or `oci://`.
- Default: none.
- Example:

```toml
[[bindings]]
name = "rpc-provider"
wit = "file://wit/interfaces/rpc/package.wit"
```

- Validation error notes: unsupported schemes fail validation. Legacy `"<package>/<interface>"` format is not supported and fails validation.

### The `imago update` expansion behavior

- Type: behavior note
- Required/Optional: Applies whenever `[[bindings]]` is configured.
- Accepted values / Constraints: `imago update` resolves the referenced WIT package and expands all interfaces into `manifest.bindings[]` entries in the form `{ "name": "<service>", "wit": "<package>/<interface>" }`.
- Default: if omitted, `manifest.bindings=[]` and runtime authorization is deny-by-default.
- Example:

```toml
[[bindings]]
name = "rpc-provider"
wit = "warg://acme:rpc-api@0.1.0"
```

- Validation error notes: for remote sources (`warg://`, `oci://`), package verification uses the resolved top-level package name rather than the literal package token in the URL.

<a id="the-dependencies-section"></a>
## The [[dependencies]] section

This section defines plugin dependencies and their resolution sources.

### The `name` field

- Type: `string`
- Required/Optional: Required in each `[[dependencies]]` entry.
- Accepted values / Constraints: ASCII letters/digits plus `.`, `_`, `-`, `:`, `/`; path components must be normal (no absolute prefix, `./`, `../`, or drive prefix).
- Default: none.
- Example:

```toml
[[dependencies]]
name = "acme:plugin/example"
version = "0.1.0"
kind = "wasm"
```

- Validation error notes: invalid path-like components fail validation.

### The `version` field

- Type: `string`
- Required/Optional: Required in each `[[dependencies]]` entry.
- Accepted values / Constraints: semantic version string used by resolution workflows.
- Default: none.
- Example:

```toml
version = "0.1.0"
```

- Validation error notes: missing version fails validation.

### The `kind` field

- Type: `string` enum
- Required/Optional: Required in each `[[dependencies]]` entry.
- Accepted values / Constraints: one of `native` or `wasm`.
- Default: none.
- Example:

```toml
kind = "wasm"
```

- Validation error notes: unknown values fail validation.

### The `wit` field

- Type: `string` or `table`
- Required/Optional: Optional.
- Accepted values / Constraints:
  - String form accepts `file://`, `warg://`, `oci://`.
  - Table form accepts `wit.source` (required) and `wit.registry` (optional, `warg://` only).
  - If omitted, source defaults to `warg://{name}@{version}`.
- Default: `warg://{name}@{version}` plus registry fallback resolution.
- Example:

```toml
wit = "warg://acme:plugin/example@0.1.0"
```

- Validation error notes: `https://wa.dev/...` shorthand is rejected; `wit.registry` with `oci://` sources is rejected.

### The `requires` field

- Type: `array(string)`
- Required/Optional: Optional.
- Accepted values / Constraints: each item follows the same package-name constraints as `name`.
- Default: empty array.
- Example:

```toml
requires = ["acme:shared/runtime"]
```

- Validation error notes: invalid package-name path components fail validation.

### The `component.source` field

- Type: `string`
- Required/Optional: Optional for `kind = "wasm"`; required when `wit` does not resolve to a component.
- Accepted values / Constraints: `file://`, `warg://`, or `oci://`.
- Default: none.
- Example:

```toml
[dependencies.component]
source = "oci://registry.example.com/acme/plugins/example@0.1.0"
```

- Validation error notes: missing component source for non-component WIT inputs fails update/deploy workflows.

### The `component.registry` field

- Type: `string`
- Required/Optional: Optional.
- Accepted values / Constraints: registry override for `warg://` component sources only.
- Default: registry fallback resolution.
- Example:

```toml
[dependencies.component]
registry = "wasi.dev"
```

- Validation error notes: using this field with `oci://` component sources fails validation.

### The `component.sha256` field

- Type: `string`
- Required/Optional: Optional.
- Accepted values / Constraints: sha256 digest string checked by `imago update` when provided.
- Default: none.
- Example:

```toml
[dependencies.component]
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```

- Validation error notes: digest mismatch fails dependency resolution.

### The `capabilities` field

- Type: table
- Required/Optional: Optional.
- Accepted values / Constraints: dependency-scoped capability rules for plugin call behavior.
- Default: none.
- Example:

```toml
[dependencies.capabilities]
wasi = true
```

- Validation error notes: invalid capability schema fails validation.

### Resolution, lockfile, and cache behavior

- Type: behavior note
- Required/Optional: Applies whenever `[[dependencies]]` is present.
- Accepted values / Constraints:
  - `imago update` resolves dependency WIT/component inputs and stores artifacts under `.imago/deps/<sanitized dependency>/`.
  - `imago update` regenerates `wit/deps/` and writes lock metadata into `imago.lock` (`version = 1`).
  - `imago build` requires matching lock/cache state and rebuilds `wit/deps/` from `.imago/deps` before validation.
  - `imago deploy` uses component metadata from `imago.lock` plus `.imago/deps` caches.
- Default: not applicable.
- Example:

```toml
[[dependencies]]
name = "acme:plugin/example"
version = "0.1.0"
kind = "wasm"
wit = "warg://acme:plugin/example@0.1.0"
```

- Validation error notes:
  - `dependencies[].wit.source = "file://..."` pointing into `wit/deps` is rejected.
  - Multiple dependencies resolving to the same `wit/deps` output path are rejected.
  - Missing or mismatched `imago.lock` entries (`wit_*`, `component_*`, `wit_packages`) fail build/deploy and require `imago update`.
  - For remote inputs, resolved top-level package names must match `dependencies[].name`.

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

- Validation error notes: values must be strings. This table applies only when a `warg://` source omits its registry.

<a id="legacy-sections"></a>
## Legacy sections

These sections are still accepted for compatibility but are ignored by manifest output.

### The `[vars]` section

#### The `<KEY>` field

- Type: `string` value in a table
- Required/Optional: Optional.
- Accepted values / Constraints: string values only.
- Default: empty table.
- Example:

```toml
[vars]
APP_MODE = "prod"
```

- Validation error notes: non-string values fail validation.

### The `[secrets]` section

#### The `<KEY>` field

- Type: `string` value in a table
- Required/Optional: Optional.
- Accepted values / Constraints: string values only.
- Default: empty table.
- Example:

```toml
[secrets]
SECRET_TOKEN = "change-me"
```

- Validation error notes: non-string values fail validation.

## Related source modules

- Build and manifest normalization:
  - [`crates/imago-cli/src/commands/build/mod.rs`](../crates/imago-cli/src/commands/build/mod.rs)
- Config validation helpers:
  - [`crates/imago-cli/src/commands/build/validation.rs`](../crates/imago-cli/src/commands/build/validation.rs)
- Dependency and lock resolution:
  - [`crates/imago-cli/src/commands/update/mod.rs`](../crates/imago-cli/src/commands/update/mod.rs)
  - [`crates/imago-lockfile/src/lib.rs`](../crates/imago-lockfile/src/lib.rs)
