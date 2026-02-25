# imago-protocol Internal Reference

## Scope

This document maps internal modules in `crates/imago-protocol` to protocol responsibilities.
It is implementation-facing and intentionally concise.

## Module Map

- `cbor.rs`: serde-based CBOR encode/decode wrappers.
- `envelope.rs`: common envelope shape and identifier validation.
- `error.rs`: structured error and code definitions.
- `messages/*`: per-domain request/response type definitions.
- `validate.rs`: reusable validation utilities and error type.

## Message Domains

- `hello`: negotiation request/response.
- `artifact`: deploy prepare/push/commit contracts.
- `command`: start/event/state/cancel contracts.
- `log`: logs request/chunk/end contracts.
- `rpc`: invoke request/response contracts.
- `service`: service listing contracts.
- `bindings`: certificate upload contracts.

## Validation Invariants

- non-empty required strings
- non-nil UUID identifiers
- positive numeric fields where required
- command payload and command type consistency
- mutually exclusive success/error response patterns for RPC

## Test Coverage Expectations

Tests should keep coverage for:

- serialization roundtrips
- required field rejection
- enum/shape validation failures
- wire compatibility for existing message tags

## Related Specifications

- [imago-protocol Overview](./imago-protocol.md)
- [Deploy Protocol Specification](./deploy-protocol.md)
