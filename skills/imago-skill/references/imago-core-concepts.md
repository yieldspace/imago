# Imago Core Concepts

## Purpose

Use this document for single-service workflows driven by `imago.toml`.
Use it when users ask how to run imago without compose.

## Command Surface (Core)

- `init`
- `build`
- `update`
- `deploy`
- `run`
- `stop`
- `ps`
- `logs`
- `bindings cert upload|deploy`
- `certs generate`

## Core Model

- `imago.toml` defines one service project.
- `[target.<name>]` inside `imago.toml` defines remote destination defaults.
- Typical one-service path:
  1. Prepare metadata/dependencies (`update`).
  2. Build artifacts (`build`).
  3. Deploy to imagod (`deploy`).
  4. Observe runtime (`ps`/`logs`) and operate lifecycle (`run`/`stop`).

## Target Usage

- `--target <name>` selects one target from `[target.<name>]`.
- `build` defaults to `default` target when omitted.
- `deploy`, `run`, and `stop` accept optional `--target`; use explicit target names in multi-node environments.

## Output Modes

- `--json` emits JSON Lines and overrides rich/plain UI.
- Use `--json` for automation or machine parsing.
- Use rich/plain mode for interactive operator sessions.

## Trust Responsibilities

- `certs generate` creates local client key material used by the `imago` command.
- `bindings cert upload` uploads a public key to a remote authority.
- `bindings cert deploy` copies binding trust from one authority to another.
- Treat trust operations as separate from deploy/build failures.

## Diagnosis Heuristic

1. Validate project file presence (`imago.toml`) and target naming.
2. Validate dependency/build stage (`update` and `build`).
3. Validate remote auth/trust (`certs` / `bindings cert`).
4. Validate runtime state (`ps`, `logs`, `run`, `stop`).
