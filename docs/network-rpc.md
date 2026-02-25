# Network RPC

Network RPC unifies local and remote service invocation through manager control paths.
The caller never talks directly to remote runner internals.

## Invocation Path

```mermaid
sequenceDiagram
    participant Caller as Caller Service
    participant LM as Local Manager
    participant RM as Remote Manager
    participant Runner as Target Runner

    Caller->>LM: rpc.connect / rpc.invoke
    LM->>RM: manager-control request
    RM->>Runner: runner inbound invoke
    Runner-->>RM: invoke result/error
    RM-->>LM: normalized response
    LM-->>Caller: rpc.invoke response
```

## Service Contract

- RPC-exposed services are configured as `type = "rpc"`.
- Runner startup in RPC mode is resident; function execution starts on `rpc.invoke`.
- Invocation payloads use protocol-defined CBOR fields.

## Authentication

- Daemon-side client keys are validated against allowlists.
- Known-host pinning protects remote manager identity checks.
- Certificate/key distribution helpers are available via bindings cert commands.

## Operational Notes

- Keep local and remote authority names stable.
- Use explicit target naming for predictable known-host lookups.
- Treat transport and permission failures as separate diagnosis tracks.

## Related Specifications

- [Deploy Protocol Specification](./spec/deploy-protocol.md)
- [Observability Specification](./spec/observability.md)
- [imago-protocol Specification](./spec/imago-protocol.md)
