# imagod.toml reference

This page is a practical reference for `imagod.toml`.
Source of truth is the codebase (module docs, type definitions, validation logic, and tests).

## Sections

- [The top-level fields section](#the-top-level-fields-section)
- [The `[tls]` section](#the-tls-section)
- [The `[runtime]` section](#the-runtime-section)

<a id="the-top-level-fields-section"></a>
## The top-level fields section

This section describes top-level daemon server settings.
These keys are defined at the root of `imagod.toml`; there is no `[server]` table.

### The `listen_addr` field

- Type: `string`
- Required/Optional: Optional.
- Accepted values / Constraints: must parse as `SocketAddr` at server bootstrap.
- Default: `"[::]:4443"`.
- Example:

```toml
listen_addr = "[::]:4443"
```

- Validation error notes: invalid socket addresses fail startup validation.

### The `storage_root` field

- Type: `string` (path)
- Required/Optional: Optional.
- Accepted values / Constraints: no additional load-time constraints.
- Default: resolved in this order:
  1. Explicit `storage_root` in `imagod.toml`
  2. Build-time `IMAGOD_STORAGE_ROOT_DEFAULT` (if non-empty)
  3. OS default (`/var/lib/imago` on Linux, `/usr/local/var/imago` on macOS, `C:\ProgramData\imago` on Windows, fallback `/var/lib/imago`)
- Example:

```toml
storage_root = "/var/lib/imago"
```

- Validation error notes: invalid path semantics are detected when used by runtime operations.

### The `server_version` field

- Type: `string`
- Required/Optional: Optional.
- Accepted values / Constraints: no additional load-time constraints.
- Default: `"imagod/0.1.0"`.
- Example:

```toml
server_version = "imagod/0.1.0"
```

- Validation error notes: non-string values fail validation. `compatibility_date` is a legacy key and now fails validation; protocol compatibility is negotiated via `hello.negotiate.client_version`.

<a id="the-tls-section"></a>
## The [tls] section

This section configures server key material and public-key allowlists.

### The `server_key` field

- Type: `string` (path)
- Required/Optional: Required.
- Accepted values / Constraints: load-time path syntax is accepted; runtime bootstrap loads key material from this path.
- Default: none.
- Example:

```toml
[tls]
server_key = "/absolute/path/to/server.key"
```

- Validation error notes: missing key files fail at runtime key loading.

### The `admin_public_keys` field

- Type: `array(string)`
- Required/Optional: Optional.
- Accepted values / Constraints: each item is an ed25519 raw public key in 64 hex characters; items must be unique and must not overlap with `client_public_keys`.
- Default: `[]`.
- Example:

```toml
[tls]
admin_public_keys = [
  "2222222222222222222222222222222222222222222222222222222222222222",
]
```

- Validation error notes: invalid length, non-hex values, duplicates, or overlap with client keys fail validation.

### The `client_public_keys` field

- Type: `array(string)`
- Required/Optional: Required.
- Accepted values / Constraints: each item is an ed25519 raw public key in 64 hex characters; items must be unique; empty arrays are allowed.
- Default: none.
- Example:

```toml
[tls]
client_public_keys = []
```

- Validation error notes: invalid key encoding or duplicate keys fail validation.

### The `known_public_keys` field

- Type: `table<string, string>`
- Required/Optional: Optional.
- Accepted values / Constraints: authority keys must be non-empty after trim; table values must be ed25519 raw public keys in 64 hex characters.
- Default: `{}`.
- Example:

```toml
[tls]
known_public_keys = { "rpc://node-a:4443" = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
```

- Validation error notes: empty authority names or invalid key values fail validation.

<a id="the-runtime-section"></a>
## The [runtime] section

This section controls transfer limits, worker settings, Wasmtime memory tuning, and startup behavior toggles.

### The `chunk_size` field

- Type: `integer` (`usize`)
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=8388608`.
- Default: `1048576`.
- Example:

```toml
[runtime]
chunk_size = 1048576
```

- Validation error notes: zero or values above 8 MiB fail validation.

### The `max_inflight_chunks` field

- Type: `integer` (`usize`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `16`.
- Example:

```toml
[runtime]
max_inflight_chunks = 16
```

- Validation error notes: zero fails validation.

### The `max_artifact_size_bytes` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `67108864`.
- Example:

```toml
[runtime]
max_artifact_size_bytes = 67108864
```

- Validation error notes: zero fails validation.

### The `upload_session_ttl_secs` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: no additional load-time validation.
- Default: `900`.
- Example:

```toml
[runtime]
upload_session_ttl_secs = 900
```

- Validation error notes: semantic issues are enforced by runtime behavior rather than strict load-time checks.

### The `stop_grace_timeout_secs` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `30`.
- Example:

```toml
[runtime]
stop_grace_timeout_secs = 30
```

- Validation error notes: zero fails validation.

### The `runner_ready_timeout_secs` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `3`.
- Example:

```toml
[runtime]
runner_ready_timeout_secs = 3
```

- Validation error notes: zero fails validation.

### The `runner_log_buffer_bytes` field

- Type: `integer` (`usize`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `262144`.
- Example:

```toml
[runtime]
runner_log_buffer_bytes = 262144
```

- Validation error notes: zero fails validation.

### The `epoch_tick_interval_ms` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `50`.
- Example:

```toml
[runtime]
epoch_tick_interval_ms = 50
```

- Validation error notes: zero fails validation.

### The `wasm_memory_reservation_bytes` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `67108864` (64 MiB).
- Behavior notes:
  - Passed to Wasmtime `Config::memory_reservation`.
  - Lower values reduce virtual-memory reservation but can increase relocation frequency on memory growth.
- Rollback notes:
  - To approximate prior 64-bit Wasmtime defaults, set `4294967296` (4 GiB).
- Example:

```toml
[runtime]
wasm_memory_reservation_bytes = 67108864
```

- Validation error notes: zero fails validation.

### The `wasm_memory_reservation_for_growth_bytes` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `16777216` (16 MiB).
- Behavior notes:
  - Passed to Wasmtime `Config::memory_reservation_for_growth`.
  - Controls how much adjacent reservation is appended on growth before full relocation.
- Rollback notes:
  - To approximate prior 64-bit Wasmtime defaults, set `2147483648` (2 GiB).
- Example:

```toml
[runtime]
wasm_memory_reservation_for_growth_bytes = 16777216
```

- Validation error notes: zero fails validation.

### The `wasm_memory_guard_size_bytes` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 0` (`0` is allowed).
- Default: `65536` (64 KiB).
- Behavior notes:
  - Passed to Wasmtime `Config::memory_guard_size`.
  - Smaller values reduce reserved virtual-memory address space but may reduce guard-based bounds-check elision opportunities.
- Rollback notes:
  - To approximate prior 64-bit Wasmtime defaults, set `33554432` (32 MiB).
- Example:

```toml
[runtime]
wasm_memory_guard_size_bytes = 65536
```

- Validation error notes: no additional bound check beyond TOML integer decoding.

### The `wasm_guard_before_linear_memory` field

- Type: `boolean`
- Required/Optional: Optional.
- Accepted values / Constraints: `true` or `false`.
- Default: `false`.
- Behavior notes:
  - Passed to Wasmtime `Config::guard_before_linear_memory`.
  - `false` reduces pre-linear-memory reservation footprint.
- Rollback notes:
  - To approximate prior 64-bit Wasmtime defaults, set `true`.
- Example:

```toml
[runtime]
wasm_guard_before_linear_memory = false
```

- Validation error notes: non-boolean values fail TOML decode.

### The `http_worker_count` field

- Type: `integer` (`u32`)
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=4`.
- Default: `2`.
- Behavior notes: compatibility knob retained for runtime bootstrap. Current HTTP execution path runs a single worker task.
- Example:

```toml
[runtime]
http_worker_count = 2
```

- Validation error notes: out-of-range values fail validation.

### The `http_worker_queue_capacity` field

- Type: `integer` (`u32`)
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=16`.
- Default: `4`.
- Behavior notes: this is an upper bound. Effective queue capacity is clamped by `runtime.http_queue_memory_budget_bytes / manifest.http.max_body_bytes`.
- Example:

```toml
[runtime]
http_worker_queue_capacity = 4
```

- Validation error notes: out-of-range values fail validation.

### The `http_queue_memory_budget_bytes` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `1..=67108864` (64 MiB).
- Default: `33554432` (32 MiB).
- Behavior notes:
  - Defines the total memory budget for queued HTTP request bodies.
  - If `manifest.http.max_body_bytes` is larger than this budget, service start fails with a validation error in `service.start`.
- Example:

```toml
[runtime]
http_queue_memory_budget_bytes = 33554432
```

- Validation error notes: zero and values over 64 MiB fail validation.

### The `manager_control_read_timeout_ms` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `500`.
- Example:

```toml
[runtime]
manager_control_read_timeout_ms = 500
```

- Validation error notes: zero fails validation.

### The `max_concurrent_sessions` field

- Type: `integer` (`u32`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `256`.
- Example:

```toml
[runtime]
max_concurrent_sessions = 256
```

- Validation error notes: zero fails validation.

### The `deploy_stream_timeout_secs` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`.
- Default: `15`.
- Example:

```toml
[runtime]
deploy_stream_timeout_secs = 15
```

- Validation error notes: zero fails validation.

### The `transport_keepalive_interval_secs` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`; must be strictly lower than `transport_max_idle_timeout_secs`.
- Default: `5`.
- Example:

```toml
[runtime]
transport_keepalive_interval_secs = 5
```

- Validation error notes: zero or values not lower than max idle timeout fail validation.

### The `transport_max_idle_timeout_secs` field

- Type: `integer` (`u64`)
- Required/Optional: Optional.
- Accepted values / Constraints: `>= 1`; must be strictly higher than `transport_keepalive_interval_secs`.
- Default: `180`.
- Example:

```toml
[runtime]
transport_max_idle_timeout_secs = 180
```

- Validation error notes: zero or values not higher than keepalive interval fail validation.

### The `boot_plugin_gc_enabled` field

- Type: `boolean`
- Required/Optional: Optional.
- Accepted values / Constraints: `true` or `false`.
- Default: `true`.
- Example:

```toml
[runtime]
boot_plugin_gc_enabled = true
```

- Validation error notes: non-boolean values fail validation.

### The `boot_restore_enabled` field

- Type: `boolean`
- Required/Optional: Optional.
- Accepted values / Constraints: `true` or `false`.
- Default: `true`.
- Example:

```toml
[runtime]
boot_restore_enabled = true
```

- Validation error notes: non-boolean values fail validation.

## Related source modules

- Config model and defaults:
  - [`crates/imagod-config/src/lib.rs`](../crates/imagod-config/src/lib.rs)
- Semantic validation:
  - [`crates/imagod-config/src/load/validation.rs`](../crates/imagod-config/src/load/validation.rs)
- Protocol handling and session routing:
  - [`crates/imagod-server/src/protocol_handler.rs`](../crates/imagod-server/src/protocol_handler.rs)
- Deploy/runtime orchestration:
  - [`crates/imagod-control/src/orchestrator.rs`](../crates/imagod-control/src/orchestrator.rs)
  - [`crates/imagod-control/src/service_supervisor.rs`](../crates/imagod-control/src/service_supervisor.rs)
