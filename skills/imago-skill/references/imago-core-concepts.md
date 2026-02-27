# Imago Core Concepts

## Purpose

Use this document for single-service workflows driven by `imago.toml`.
Use it when users ask how to run imago without compose.

## Command Surface (Core)

- `project init`
- `artifact build`
- `deps sync`
- `service deploy`
- `service start`
- `service stop`
- `service ls`
- `service logs`
- `trust cert upload|replicate`
- `trust client-key generate`

## Core Model

- `imago.toml` defines one service project.
- `[target.<name>]` inside `imago.toml` defines remote destination defaults.
- Typical one-service path:
  1. Prepare metadata/dependencies (`deps sync`).
  2. Build artifacts (`artifact build`).
  3. Deploy to imagod (`service deploy`).
  4. Observe runtime (`service ls`/`service logs`) and operate lifecycle (`service start`/`service stop`).

## Target Usage

- `--target <name>` selects one target from `[target.<name>]`.
- `artifact build` defaults to `default` target when omitted.
- `service deploy`, `service start`, and `service stop` accept optional `--target`; use explicit target names in multi-node environments.

## Output Modes

- `CI=true` forces plain output.
- Rich mode is used for interactive operator sessions.

## Trust Responsibilities

- `trust client-key generate` creates local client key material used by the `imago` command.
- `trust cert upload` uploads a public key to a remote authority.
- `trust cert replicate` copies binding trust from one authority to another.
- Treat trust operations as separate from deploy/build failures.

## Diagnosis Heuristic

1. Validate project file presence (`imago.toml`) and target naming.
2. Validate dependency/build stage (`deps sync` and `artifact build`).
3. Validate remote auth/trust (`trust client-key` / `trust cert`).
4. Validate runtime state (`service ls`, `service logs`, `service start`, `service stop`).
