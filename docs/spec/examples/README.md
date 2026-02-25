# Examples (`docs/spec/examples`)

This directory contains minimal JSON examples for specification validation.
Values are intentionally compact and focus on structural correctness.

| File | Purpose | Primary References |
|---|---|---|
| `manifest.valid.json` | Valid manifest including socket, wasi env, http outbound, assets, and hash | [manifest.md](../manifest.md), [deploy-protocol.md](../deploy-protocol.md) |
| `manifest.invalid.missing-required.json` | Missing required fields | [manifest.md](../manifest.md) |
| `manifest.invalid.bad-type.json` | Invalid enum value in `type` | [manifest.md](../manifest.md) |
| `manifest.invalid.hash-mismatch.json` | Digest mismatch example | [manifest.md](../manifest.md) |
| `manifest.invalid.wasi-env-shape.json` | Invalid `wasi.env` shape | [manifest.md](../manifest.md) |
| `rpc.invoke.request.json` | `rpc.invoke` request envelope | [imago-protocol.md](../imago-protocol.md), [deploy-protocol.md](../deploy-protocol.md) |
| `rpc.invoke.response.success.json` | Successful `rpc.invoke` response | [imago-protocol.md](../imago-protocol.md), [deploy-protocol.md](../deploy-protocol.md) |
| `rpc.invoke.response.error.json` | Error `rpc.invoke` response | [imago-protocol.md](../imago-protocol.md), [deploy-protocol.md](../deploy-protocol.md) |
