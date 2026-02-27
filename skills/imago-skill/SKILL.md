---
name: imago-skill
description: Explain imago fundamentals and guide both core imago workflows (project init/artifact build/deps sync/service deploy/service start/service stop/service ls/service logs/trust cert/trust client-key) and imago stack workflows (build/sync/deploy/logs/ls) for this repository. Use when users ask how imago works, how to operate single-service or multi-service deployments, or how to structure imago.toml and imago-compose.toml.
---

# Imago Skill

## Overview

Teach imago fundamentals and operational workflows for both single-service (`imago` core commands) and multi-service (`imago stack`) use cases.
Route each request to the shortest reliable command sequence, then diagnose failures by matching structural constraints first.

## Command families and when to use

- Use core imago commands (`project init/artifact build/deps sync/service deploy/service start/service stop/service ls/service logs`) when the user is working on one service project rooted by one `imago.toml`.
- Use `stack` commands (`stack build/sync/deploy/logs/ls`) when the user is orchestrating multiple services via `imago-compose.toml`.
- Use `trust cert` commands when trust data must be uploaded or copied between authorities.
- Use `trust client-key generate` when the user needs a local client key for `imago` authentication.

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
  - core: `deps sync -> artifact build -> service deploy -> service ls/service logs`
  - compose: `stack sync -> stack build -> stack deploy -> stack ls/logs`

## Core imago playbooks

### Playbook A: initialize a service project

```bash
imago project init .
imago project init services/example --lang rust
```

### Playbook B: first deployment for one service

```bash
imago deps sync
imago artifact build --target <target>
imago service deploy --target <target> --detach
imago service ls --target <target>
imago service logs <service-name> --tail 200
```

### Playbook C: lifecycle operations

```bash
imago service start <service-name> --target <target> --detach
imago service stop <service-name> --target <target>
```

### Playbook D: certificate and trust operations

```bash
imago trust client-key generate --out-dir certs
imago trust cert upload <public_key_hex> --to <remote-authority>
imago trust cert replicate --from <remote-authority> --to <remote-authority>
```

## Compose playbooks

### Playbook E: initial deployment of a profile

```bash
imago stack sync <profile>
imago stack build <profile> --target <target>
imago stack deploy <profile> --target <target>
imago stack ls <profile> --target <target>
imago stack logs <profile> --target <target> --tail 200
```

### Playbook F: inspect status and logs only

```bash
imago stack ls <profile> --target <target>
imago stack logs <profile> --target <target> --name <service-name> --tail 200
imago stack logs <profile> --target <target> --follow --tail 200
```

## Troubleshooting matrix

- Classify first:
  - Configuration mismatch: missing file, missing profile/target, invalid manifest path, empty required fields.
  - Runtime failure: remote connectivity, auth/trust mismatch, service startup/runtime errors.

- Core imago common errors:
  - Missing/invalid `imago.toml` context.
  - Unknown target in `[target.<name>]`.
  - Authentication failures requiring `trust client-key`/`trust cert` updates.

- Compose common errors:
  - `failed to read compose file` / `failed to parse compose file`.
  - `profile '<name>' is not defined` / `compose config '<name>' is not defined`.
  - `service.imago must point to imago.toml` / `service.imago file does not exist`.
  - `target '<name>' is not defined` / `target '<name>' is missing required key: remote`.
  - `stack logs --name must not be empty`.

- Trust/cert hints:
  - Use `trust client-key generate` to create local client key material.
  - Use `trust cert upload/replicate` to align authority trust.

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
