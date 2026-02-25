# Imago Core Recipes

## Purpose

Use this document for repository-aligned, single-service command sequences.
These recipes align with `QUICKSTART.md` and current CLI shape.

## Recipe 1: Initialize project metadata

```bash
imago init .
imago init services/example --lang rust
```

## Recipe 2: Local example (single service)

Run from `examples/local-imagod`.

### Terminal 1

```bash
imagod --config imagod.toml
```

### Terminal 2

```bash
imago deploy --target default --detach
imago logs local-imagod-app --tail 200
```

### Success signal

- Logs include a line like `local-imagod-app started`.

## Recipe 3: Standard one-service lifecycle

```bash
imago update
imago build --target default
imago deploy --target default --detach
imago ps --target default
```

`deploy` already starts/replaces the service. Use the following only when you intentionally want to restart:

```bash
imago stop <service-name> --target default
imago run <service-name> --target default --detach
```

## Recipe 4: Cert and binding trust setup

### Generate local client key material

```bash
imago certs generate --out-dir certs
```

### Upload a binding public key

```bash
imago bindings cert upload <public_key_hex> --to <remote-authority>
```

### Copy binding trust between authorities

```bash
imago bindings cert deploy --from <remote-authority> --to <remote-authority>
```

## Quick Error Mapping

- Deploy/build failure with target mismatch:
  - Verify target name exists in `imago.toml`.

- Logs/ps show no expected service state:
  - Re-run deploy and check daemon side logs.

- Auth/trust failure:
  - Re-check `certs generate` output and `bindings cert` authority arguments.
