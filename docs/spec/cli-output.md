# CLI Output Specification

## Purpose

This document defines user-facing and machine-facing output contracts for `imago` CLI commands.

## Output Modes

The CLI chooses output mode in this order:

1. `--json` => JSON lines mode
2. `CI=true` => plain text mode
3. otherwise => rich interactive mode

## JSON Line Contracts

### `command.summary`

Emitted at successful completion for summary-capable commands.

Required fields:

- `type`
- `command`
- `status`
- `duration_ms`
- `timestamp`
- `meta`
- `error`

### `command.error`

Emitted on failure.

Required fields:

- `type`
- `command`
- `message`
- `stage`
- `code`

### `log.line`

Emitted by `logs --json` for each line-style log event.

Required fields:

- `type`
- `name`
- `stream`
- `timestamp`
- `log`

### `ps --json` entries

Service status output is line-oriented and includes service name, state, release hash, and start time fields.

## Plain/Rich Mode Expectations

- Human-readable summaries MUST match command outcomes.
- Errors SHOULD include actionable diagnostics and stage context.
- Streamed logs SHOULD preserve service and stream identity.

## Related Specifications

- [Observability Specification](./observability.md)
- [Deploy Protocol Specification](./deploy-protocol.md)
