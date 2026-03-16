# imago-usb native plugin

`imago:usb@0.2.0` provides native USB access APIs backed by `rusb` (libusb).

## Features

- Linux-focused USB operations (`open`, `claim`, `control`, `bulk`, `interrupt`)
- `wasi-usb` parity additions used by this plugin:
  - openable-device enumeration
  - device descriptors and configuration descriptors
  - reset and active-configuration switching
  - alternate setting control
  - isochronous in/out transfer
  - device connection event polling
- Strict `resources.usb.paths` allowlist enforcement
- Runtime limits surfaced by `get-limits`

## Runtime model

- Backend is fixed to `rusb` (libusb).
- One dedicated worker thread is created per opened `device` resource.
- Host calls are sent to the worker via bounded `tokio::sync::mpsc` and completed via `oneshot`.
- `bulk-read` lazily starts one producer thread per `(claimed-interface, endpoint-in)` pair.
- Producer threads write fixed-size chunks into bounded ring buffers and drop oldest chunks on overflow.

## Requirements

- Linux runtime target (non-Linux returns `operation-not-supported`)
- Read/write permission to configured USB device nodes
- System libusb by default (`vendored-libusb` feature is optional for build environments that need bundled libusb)

## `imago.toml` configuration

```toml
[[dependencies]]
name = "imago:usb"
version = "0.2.0"
kind = "native"
wit = "file://../../plugins/imago-usb/wit"

[resources.usb]
paths = [
  "/dev/bus/usb/001/001",
  "/dev/bus/usb/001/002",
]
max_transfer_bytes = 1048576
max_timeout_ms = 30000
max_paths = 128
bulk_ring_chunk_bytes = 16384
bulk_ring_slots = 16

[capabilities.deps]
"imago:usb" = ["*"]
```

`paths` is required. An empty array is valid and means all open operations are denied.
If omitted, `bulk_ring_chunk_bytes` defaults to `min(16384, max_transfer_bytes)`.

## WIT import example

```wit
package example:usb-client;

world plugin-imports {
    import imago:usb/provider@0.2.0;
    import imago:usb/device@0.2.0;
    import imago:usb/usb-interface@0.2.0;
}
```

## Rust call examples (`wasm32-wasip2`)

### Enumerate and open allowlisted devices

```rust
let paths = imago::usb::provider::list_openable_paths();
let openable = imago::usb::provider::list_openable_devices()?;
let limits = imago::usb::provider::get_limits();

if let Some(dev) = openable.first() {
    let device = imago::usb::provider::open_device(&dev.path)?;
    let iface = device.claim_interface(0)?;
    let _ = iface.bulk_out(0x01, &[0x10, 0x20], 1000)?;
}
```

### Read descriptors and switch configuration

```rust
let device = imago::usb::provider::open_device("/dev/bus/usb/001/001")?;
let desc = device.device_descriptor()?;
let configs = device.configurations()?;
let active = device.active_configuration()?;

if active != 1 {
    device.select_configuration(1)?;
}

let iface = device.claim_interface(0)?;
iface.set_alternate_setting(1)?;
let _alt = iface.alternate_setting()?;
```

### Poll connection events and use isochronous transfer

```rust
let event = imago::usb::provider::poll_device_event(5000)?;

let device = imago::usb::provider::open_device("/dev/bus/usb/001/001")?;
let iface = device.claim_interface(1)?;
let payload = iface.isochronous_in(0x81, 1024, 8, 1000)?;
let _written = iface.isochronous_out(0x01, &payload, 8, 1000)?;
```

`isochronous_in` returns the actual payload bytes reported by each packet descriptor,
concatenated in packet order. The aggregate libusb `actual_length` field is not used for
isochronous transfers.

### Consume bulk IN chunks from ring buffer

```rust
let device = imago::usb::provider::open_device("/dev/bus/usb/001/001")?;
let iface = device.claim_interface(0)?;

loop {
    let chunk = iface.bulk_read(0x81, 200)?;
    if chunk.is_empty() {
        continue;
    }
    process_samples(&chunk);

    let stats = iface.bulk_read_stats(0x81)?;
    if stats.dropped_chunks > 0 {
        eprintln!(
            "bulk ring overflow: dropped_chunks={}, dropped_bytes={}",
            stats.dropped_chunks, stats.dropped_bytes
        );
    }
}
```

## Resource validation rules

`resources.usb` is validated at startup.

- Missing `resources.usb` is an error
- Missing `paths`, non-array `paths`, or non-string entries are errors
- Paths must be absolute and must not be empty or contain NUL
- Duplicates after normalization are errors
- `max_transfer_bytes` must be within `1..=8388608`
- `max_timeout_ms` must be within `1..=120000`
- `max_paths` must be within `0..=256`
- `bulk_ring_chunk_bytes` must be within `1..=max_transfer_bytes`
- `bulk_ring_slots` must be within `1..=256`
- `bulk_ring_chunk_bytes * bulk_ring_slots` must be within `1..=67108864`
- `paths.len() > max_paths` is an error

## Error behavior notes

- Path allowlist rejection always returns `usb-error.not-allowed`.
- Timeout and cancelled transfer paths are normalized to `usb-error.timeout`.
- On Linux hot-unplug paths are mapped to `usb-error.disconnected`.
