# imago-v4l2 native plugin

`imago:v4l2@0.1.0` provides native Linux V4L2 capture APIs backed by `v4l2r`.

## Features

- Strict `resources.v4l2.paths` allowlist enforcement
- One worker thread per opened `device` resource
- One active MMAP capture stream per opened device
- MJPEG capture mode enumeration via `VIDIOC_ENUM_FMT/FRAMESIZES/FRAMEINTERVALS`
- Stepwise and continuous mode descriptors expand to exact modes with a fail-closed 4096-mode cap per device
- USB metadata lookup for `/dev/video*` nodes via sysfs, with fields left as `0` when metadata is unavailable

## Scope

- Linux only
- Capture-only
- MJPEG-only
- No hotplug support
- No multi-stream support
