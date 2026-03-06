# QUICKSTART

## Goal

This guide helps you:

1. Install `imago` CLI and `imagod`.
2. Create a brand-new project with `imago project init`.
3. Build and run the generated template service on local `imagod`.

This quickstart does not require cloning the `imago` repository.

## Install Imago CLI

Choose one installation method:

Option A:
```bash
curl -sSf https://cli.imago.sh | sh
```

This installer defaults to the latest stable `imago` release.
If only prereleases are available, use `--prerelease`. To pin an exact build, use `--tag imago-vX.Y.Z`.
Interactive terminals show a confirmation prompt before installation.
For automation or unattended runs, pass `-y` (for example, `curl -sSf https://cli.imago.sh | sh -s -- -y`).
Repository-local fallback:
```bash
curl -fsSL https://raw.githubusercontent.com/yieldspace/imago/main/scripts/install_imago.sh | sh
```

Option B:
```bash
cargo install imago-cli --git https://github.com/yieldspace/imago
```

## Install imagod

Choose one installation method:

Option A:
```bash
curl -fsSL https://raw.githubusercontent.com/yieldspace/imago/main/scripts/install_imagod.sh | sh
```

This installer defaults to the latest stable `imagod` release.
If only prereleases are available, use `--prerelease`. To pin an exact build, use `--tag imagod-vX.Y.Z`.
Interactive terminals show a confirmation prompt before installation.
For automation or unattended runs, pass `-y` (for example, `curl -fsSL https://raw.githubusercontent.com/yieldspace/imago/main/scripts/install_imagod.sh | sh -s -- -y`).

Option B:
```bash
cargo install --git https://github.com/yieldspace/imago imagod
```

## Create a New Project from Template

```bash
mkdir -p ~/imago-quickstart
cd ~/imago-quickstart
imago project init app --template rust
cd app
```

## Install Wasm Target

```bash
rustup target add wasm32-wasip2
```

## Generate Local Key Material

Generate one local keypair:

```bash
CI=true imago trust client-key generate --out-dir certs
```

From the command output, copy the value shown as:

```text
client_public_key_hex=<YOUR_CLIENT_PUBLIC_KEY_HEX>
```

## Configure `imago.toml`

Write the following content to `imago.toml` in your project root:

```toml
"$schema" = "https://raw.githubusercontent.com/yieldspace/imago/main/schemas/imago.schema.json"

name = "example-service"
main = "target/wasm32-wasip2/release/example-service.wasm"
type = "cli"

[build]
command = "CARGO_TARGET_DIR=target cargo build --target wasm32-wasip2 --release"

[capabilities]
wasi = true

[target.default]
remote = "127.0.0.1:4443"
server_name = "localhost"
client_key = "certs/client.key"
```

## Configure `imagod.toml`

For this local quickstart only, we reuse the generated key as both server key and allowed client key.
Do not use this setup for production.

Install imagod:

```bash
# For server
curl -sSf https://install.imago.sh | sh
# For local
cargo install imagod --git https://github.com/yieldspace/imago
```

Write the following content to `imagod.toml` in your project root:

```toml
"$schema" = "https://raw.githubusercontent.com/yieldspace/imago/main/schemas/imagod.schema.json"

listen_addr = "127.0.0.1:4443"
storage_root = ".imagod-data"
server_version = "imagod/local-quickstart"

[tls]
server_key = "certs/client.key"
admin_public_keys = []
client_public_keys = ["<YOUR_CLIENT_PUBLIC_KEY_HEX>"]
```

## Run Local Example

```bash
# Terminal 1
# Start daemon
cd ~/imago-quickstart/app
imagod --config imagod.toml
```

```bash
# Terminal 2
# Build, deploy, and stream logs
cd ~/imago-quickstart/app
imago service deploy
```

## Success Check

After running the log command, confirm output includes a line similar to:

```text
example-service stdout | Hello, World!
```

## Troubleshooting

If deploy fails with a known-host mismatch, remove stale `localhost:4443` / `127.0.0.1:4443`
entries from `~/.imago/known_hosts` and retry.

## Next Steps

- `imago service ls --target default`
- `imago service stop example-service --target default`
