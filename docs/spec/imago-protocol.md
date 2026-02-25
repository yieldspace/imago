# imago-protocol Specification (Overview)

## Purpose

`imago-protocol` defines shared wire types and validation rules used by `imago-cli` and `imagod`.

## Responsibilities

- CBOR encode/decode helpers
- protocol envelope type
- message payload types
- structured error model
- payload validation trait and errors

## Non-Responsibilities

- transport connection management
- session framing implementation details
- daemon orchestration logic

## Public API Map

- `cbor`: binary encoding helpers
- `envelope`: common envelope and id semantics
- `messages`: request/response payloads and message type enum
- `error`: structured error contracts
- `validate`: validation trait and reusable checks

## Key Contract Points

- Envelope identifiers MUST be non-nil UUID values.
- `command.start` payload MUST match command type.
- `state.response` only carries in-flight states.
- `rpc.invoke` response MUST be either success payload or error payload.
- `services.list` supports optional name filters.

## Related Specifications

- [Deploy Protocol Specification](./deploy-protocol.md)
- [Observability Specification](./observability.md)
- [imago-protocol Internal Reference](./imago-protocol-internals.md)
