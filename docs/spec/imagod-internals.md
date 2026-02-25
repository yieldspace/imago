# imagod Internal Architecture Reference

## Scope

This document maps internal modules and runtime boundaries for `imagod` and `imagod-*` crates.
For external protocol contracts, see the top-level specifications.

## Process Phases

1. Load and validate daemon configuration.
2. Initialize transport and protocol handler dependencies.
3. Accept sessions and dispatch protocol envelopes.
4. Orchestrate artifact, release, and process lifecycle operations.
5. Serve runtime status, logs, and RPC invocation paths.

## Internal Component Map

- `imagod-config`: config decoding, validation, and config update helpers.
- `imagod-server`: transport bootstrap and protocol handler/session routing.
- `imagod-control`: artifact store, orchestrator, operation state, service supervisor.
- `imagod-ipc`: manager-runner IPC message types and transport adapters.
- `imagod-runtime*`: runner bootstrap, ingress, runtime abstraction, backend execution.

## Major Runtime Boundaries

- Protocol boundary: `ProtocolEnvelope` decode/encode and message dispatch.
- Artifact boundary: staged upload session to committed release transition.
- Supervisor boundary: service launch/stop/readiness lifecycle management.
- Runner boundary: bootstrap decode, runtime initialization, and inbound invoke handling.
- Capability boundary: app/dependency/WASI checks for allowed calls.

## Operational Invariants

- Runner startup must produce readiness within configured timeout windows.
- Stop operations must honor grace timeout before forced termination.
- Log retention is memory-bounded.
- Boot restore is best-effort and scoped by restart policy.
- Plugin artifacts must pass digest checks before runtime use.

## Failure Model

Failures are returned as structured errors with stage metadata.
Typical stages include:

- `transport.setup`
- `config.load`
- `orchestration`
- `service.start`
- `service.stop`
- `service.control`

## Related Specifications

- [imagod Overview](./imagod.md)
- [Configuration Specification](./config.md)
- [Deploy Protocol Specification](./deploy-protocol.md)
