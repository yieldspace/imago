# imagod Server Specification (Overview)

## Purpose

`imagod` is the network-facing deployment and runtime manager for Imago services.
This document defines external behavior at a system level.

## Topology

```mermaid
flowchart LR
    A["imago-cli"] --> B["imagod protocol handler"]
    B --> C["orchestrator"]
    C --> D["service supervisor"]
    D --> E["runner process"]
    E --> F["runtime backend"]
```

## Responsibilities

- Accept authenticated protocol sessions
- Validate and stage artifacts
- Promote releases and manage active state
- Launch and supervise runner processes
- Route command, state, log, and RPC operations
- Enforce capability and binding policy at runtime boundaries

## Configuration Surface

`imagod` behavior is driven by `imagod.toml` for:

- listen and storage paths
- TLS key material and key allowlists
- transfer/session/runtime limits
- transport keepalive and idle timeout policy
- boot-time GC and restore toggles

## Process Model

- Manager process handles protocol and orchestration.
- Runner process executes component runtime workloads.
- Internal control channels coordinate readiness, invocation, and lifecycle updates.

## State and Observability

- command lifecycle events are streamed via protocol messages.
- status snapshots are served via state and service list requests.
- logs can include live and retained in-memory data within configured limits.

## Related Specifications

- [Configuration Specification](./config.md)
- [Deploy Protocol Specification](./deploy-protocol.md)
- [Observability Specification](./observability.md)
- [imagod Internal Reference](./imagod-internals.md)
