---
name: imago-skill
description: Explain imago fundamentals and guide both core imago workflows (init/build/update/deploy/run/stop/ps/logs/bindings/certs) and imago compose workflows (build/update/deploy/logs/ps) for this repository. Use when users ask how imago works, how to operate single-service or multi-service deployments, or how to structure imago.toml and imago-compose.toml.
---

# Imago Skill

## Overview

Teach imago fundamentals and operational workflows for both single-service (`imago` core commands) and multi-service (`imago compose`) use cases.
Route each request to the shortest reliable command sequence, then diagnose failures by matching structural constraints first.

## Command families and when to use

- Use core imago commands (`init/build/update/deploy/run/stop/ps/logs`) when the user is working on one service project rooted by one `imago.toml`.
- Use `compose` commands (`compose build/update/deploy/logs/ps`) when the user is orchestrating multiple services via `imago-compose.toml`.
- Use `bindings cert` commands when trust data must be uploaded or copied between authorities.
- Use `certs generate` when the user needs a local client key for imago-cli authentication.

## Concept model (imago.toml vs imago-compose.toml)

- `imago.toml` is the single-service source of truth for build/deploy/runtime metadata.
- `imago-compose.toml` orchestrates multiple service projects by referencing multiple `imago.toml` files.
- Compose routing map:
  - `[profile.<profile_name>]` selects one compose config via `config = "<compose_config_name>"`.
  - `[[compose.<compose_config_name>.services]]` lists services with `imago = "<path-to-imago.toml>"`.
  - `[target.<target_name>]` defines remote access (`remote`, optional `server_name`, optional `client_key`).
- Compose runs are single-target per invocation. For cross-node flows, run commands per profile/target pair.

## Workflow decision guide

1. Identify intent.
- Concept question: explain model and naming first.
- Operation question: propose concrete commands.
- Failure question: map stderr to likely constraint/runtime class.

2. Classify scope.
- Single service: choose core imago playbooks.
- Multi service: choose compose playbooks.

3. Confirm minimum context before commands.
- For core: ensure working directory contains `imago.toml`.
- For compose: ensure working directory contains `imago-compose.toml` and requested `profile`/`target` exist.
- For trust/cert operations: confirm authorities and key paths.

4. Execute minimal safe sequence.
- First deployment default:
  - core: `update -> build -> deploy -> ps/logs`
  - compose: `compose update -> compose build -> compose deploy -> compose ps/logs`

## Core imago playbooks

### Playbook A: initialize a service project

```bash
cargo run -p imago-cli -- init .
cargo run -p imago-cli -- init services/example --lang rust
```

### Playbook B: first deployment for one service

```bash
cargo run -p imago-cli -- update
cargo run -p imago-cli -- build --target <target>
cargo run -p imago-cli -- deploy --target <target> --detach
cargo run -p imago-cli -- ps --target <target>
cargo run -p imago-cli -- logs <service-name> --tail 200
```

### Playbook C: lifecycle operations

```bash
cargo run -p imago-cli -- run <service-name> --target <target> --detach
cargo run -p imago-cli -- stop <service-name> --target <target>
```

### Playbook D: certificate and trust operations

```bash
cargo run -p imago-cli -- certs generate --out-dir certs
cargo run -p imago-cli -- bindings cert upload <public_key_hex> --to <remote-authority>
cargo run -p imago-cli -- bindings cert deploy --from <remote-authority> --to <remote-authority>
```

## Compose playbooks

### Playbook E: initial deployment of a profile

```bash
cargo run -p imago-cli -- compose update <profile>
cargo run -p imago-cli -- compose build <profile> --target <target>
cargo run -p imago-cli -- compose deploy <profile> --target <target>
cargo run -p imago-cli -- compose ps <profile> --target <target>
cargo run -p imago-cli -- compose logs <profile> --target <target> --tail 200
```

### Playbook F: inspect status and logs only

```bash
cargo run -p imago-cli -- compose ps <profile> --target <target>
cargo run -p imago-cli -- compose logs <profile> --target <target> --name <service-name> --tail 200
cargo run -p imago-cli -- compose logs <profile> --target <target> --follow --tail 200
```

## Troubleshooting matrix

- Classify first:
  - Configuration mismatch: missing file, missing profile/target, invalid manifest path, empty required fields.
  - Runtime failure: remote connectivity, auth/trust mismatch, service startup/runtime errors.

- Core imago common errors:
  - Missing/invalid `imago.toml` context.
  - Unknown target in `[target.<name>]`.
  - Authentication failures requiring `certs`/`bindings` updates.

- Compose common errors:
  - `failed to read compose file` / `failed to parse compose file`.
  - `profile '<name>' is not defined` / `compose config '<name>' is not defined`.
  - `service.imago must point to imago.toml` / `service.imago file does not exist`.
  - `target '<name>' is not defined` / `target '<name>' is missing required key: remote`.
  - `compose logs --name must not be empty`.

- Trust/cert hints:
  - Use `certs generate` to create local client key material.
  - Use `bindings cert upload/deploy` to align authority trust.

## When to read references

- Core concepts and scope:
  - [`references/imago-core-concepts.md`](references/imago-core-concepts.md)
- Core operational recipes:
  - [`references/imago-core-recipes.md`](references/imago-core-recipes.md)
- Compose concepts and constraints:
  - [`references/compose-concepts.md`](references/compose-concepts.md)
- Compose repository recipes:
  - [`references/compose-recipes.md`](references/compose-recipes.md)

Keep this file for routing/decision logic and keep long command examples in references.
