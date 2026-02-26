# CLI Output Contract

This document defines the user-facing output contract for `imago` CLI commands.

## Goals

- Keep command output readable without repeated wording.
- Make success/failure termination explicit.
- Provide deploy/run/stop/logs context in a human-readable form.

## Output Structure

### Progress lines

- Commands print natural progress text such as:
  - `deploy: loading target configuration...`
  - `deploy: connecting target...`
- Progress wording should avoid machine tags like `[start]` or `[progress]`.

### Success termination

- Commands print one success line:
  - `<command> succeeded`
- Then they may print detail lines in key-value format:
  - `  service: <value>`
  - `  target: <value>`
- Detail keys come from `CommandResult.meta` and are emitted in stable key order.

### Failure termination

- Commands print one short failure line:
  - `<command> failed (<short reason>)`
- Detailed diagnostics are printed as:
  - `error: ...`
  - `caused by:`
  - `hint:`

## Command-specific rules

### deploy

- On success, output should include enough information to answer:
  - what was deployed (`service`, `deploy_id`)
  - where it was deployed (`target`, `authority`, `resolved`)
  - when deployment completed (`deployed_at`)

### logs

- `logs` does not print success termination lines (`logs succeeded`) in either follow or non-follow mode.
- `logs` does not print success detail lines from `CommandResult.meta`.
- For `logs --follow`, progress spinners must be cleared before streamed log lines begin.
- Streamed log lines use `app | message` format (no `stdout`/`stderr`/`composite` label in the prefix).
- The `|` column is aligned to the longest observed app name width.
  - Example: `api         | started`
  - Example: `longer-name | listening on 0.0.0.0:8080`
- `logs --with-timestamp` and `compose logs --with-timestamp` switch the line format to:
  - `app | <local RFC3339 with offset> <message>`
  - Example: `api | 2026-02-26T09:12:03+09:00 started`
- The timestamp value represents the original log-read time recorded by imagod.
- When `--with-timestamp` is not specified, the default output remains `app | message`.
- On failure, `logs` still prints the standard failure line and diagnostics (`error:`, `caused by:`, `hint:`).

## Stability notes

- This contract is part of the CLI UX and should be updated with tests when changed.
- Any wording changes should keep the three-level model:
  - progress
  - terminal status
  - optional detail/diagnostics
