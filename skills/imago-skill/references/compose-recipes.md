# Compose Recipes

## Purpose

Use this document for repository-specific multi-service execution examples and success checks.
Base examples on `examples/imago-compose-bindings`.
For one-service recipes, use `imago-core-recipes.md`.

## Recipe 1: Local one-node compose flow

Run from `examples/imago-compose-bindings`.

### Terminal A

`prepare` in `stack build prepare --target default` is a profile argument to the `build` subcommand, not a separate top-level stack command.

```bash
imago stack build prepare --target default
imago stack sync dev
imago stack build dev --target default
imagod --config imagod.toml
```

### Terminal B

```bash
imago stack deploy dev --target default
imago stack logs dev --target default --name cli-client --tail 200
```

### Success signal

- `stack logs ... --name cli-client` contains `acme:clock/api.now =>`.

## Recipe 2: Docker cross-imagod compose flow (alice/bob)

Run from `examples/imago-compose-bindings/docker`.
Assume the `imago` binary is available in the `imago-deployer` container.

### Start environment

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e up --build -d imagod-alice imagod-bob imago-deployer
```

### Deploy greeter to bob

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack sync greeter

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack build greeter --target bob

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack deploy greeter --target bob
```

### Deploy client to alice

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack sync client

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack build client --target alice

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack deploy client --target alice
```

### Inspect logs before and after trust distribution

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack logs client --target alice --name cli-client --tail 200

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago trust cert replicate --from imagod-alice:4443 --to imagod-bob:4443

docker compose --project-name imago-compose-bindings-alice-bob-e2e \
  exec -T --workdir /workspace/examples/imago-compose-bindings/docker imago-deployer \
  imago stack logs client --target alice --name cli-client --tail 200
```

### Success signal

- Before cert deployment: connection failure is expected.
- After `trust cert replicate`: logs contain `acme:clock/api.now =>`.

### Teardown

```bash
docker compose --project-name imago-compose-bindings-alice-bob-e2e down --remove-orphans
```

## Error Signatures and First Response

- `failed to read compose file`:
  - Move to the directory containing `imago-compose.toml`.

- `profile '<name>' is not defined in imago-compose.toml`:
  - Select an existing profile from `[profile.*]`.

- `target '<name>' is not defined in imago-compose.toml`:
  - Select an existing target from `[target.*]`.

- `service.imago must point to imago.toml`:
  - Fix `[[compose.<config>.services]].imago` to a valid `imago.toml` path.

- `stack logs --name must not be empty`:
  - Provide non-empty `--name` or remove `--name`.

- TOFU pin mismatch for `localhost:4443`:
  - Remove only the conflicting entry from `$HOME/.imago/known_hosts` and retry.

## Teaching Notes

- Prefer profile-specific commands over broad guesses.
- Repeat target context in every stack command explanation.
- Keep guidance deterministic: `read config -> choose profile/target -> run sequence -> verify logs`.
