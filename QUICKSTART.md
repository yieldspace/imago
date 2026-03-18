# QUICKSTART

## Goal

This guide helps you:

1. Install `imago` and `imagod`.
2. Create a brand-new project with `imago project init`.
3. Build and run the generated template service on local `imagod`.

This quickstart does not require cloning the `imago` repository.

## Install `imago`

```bash
curl -sSLf https://cli.imago.sh | sh
```

## Install `imagod`

```bash
curl -sSLf https://install.imago.sh | sh
```

This guide uses manual local startup with `imagod --config ./imagod.toml`.
It does not configure a system service.

## Create a New Project from Template

```bash
mkdir -p ~/imago-quickstart
cd ~/imago-quickstart
imago project init app --template rust
cd app
```

## Install the Wasm Target

```bash
rustup target add wasm32-wasip2
```

## Generate Local Key Material

```bash
imago trust client-key generate --out-dir certs
```

For this local quickstart, reuse `certs/client.key` as the daemon `tls.server_key`.
Do not use this shortcut for production trust setup.

## Update `imago.toml`

Replace the generated `imago.toml` with:

```toml
"$schema" = "https://raw.githubusercontent.com/yieldspace/imago/main/schemas/imago.schema.json"

name.cargo = true

main = "target/wasm32-wasip2/release/example-service.wasm"
type = "cli"

[build]
command = "CARGO_TARGET_DIR=target cargo build --target wasm32-wasip2 --release"

[capabilities]
wasi = true

[target.default]
remote = "ssh://localhost?socket=/tmp/imago-quickstart-imagod.sock"
```

`name.cargo = true` reads `./Cargo.toml` `[package].name` from the same project root as `imago.toml`.
If you prefer a literal value, keep using `name = "example-service"` instead.
This lookup does not search parent directories; a missing sibling file or missing `[package].name` fails closed.

Loopback targets without `user@` or `:port` such as `ssh://localhost?...` connect directly to
the local control socket.

For a remote host, use:

`remote = "ssh://user@your-host?socket=/run/imago/imagod.sock"`

The CLI runs `ssh <host> imagod proxy-stdio`, and OpenSSH handles authentication and host verification.

## Create `imagod.toml`

Create `imagod.toml` in the project root:

```toml
"$schema" = "https://raw.githubusercontent.com/yieldspace/imago/main/schemas/imagod.schema.json"

listen_addr = "127.0.0.1:4443"
control_socket_path = "/tmp/imago-quickstart-imagod.sock"
storage_root = ".imagod-data"
server_version = "imagod/local-quickstart"

[tls]
server_key = "certs/client.key"
client_public_keys = []
```

`control_socket_path` must match the `?socket=` query in `imago.toml`.

## Run the Quickstart

```bash
# Terminal 1
cd ~/imago-quickstart/app
imagod --config ./imagod.toml
```

```bash
# Terminal 2
cd ~/imago-quickstart/app
imago service deploy --target default --detach
imago service logs example-service --tail 200
```

## Success Check

After running the log command, confirm output includes a line similar to:

```text
example-service stdout | Hello, World!
```

## Troubleshooting

- Confirm `imagod` is still running in Terminal 1.
- Confirm `imago.toml` uses `remote = "ssh://localhost?socket=/tmp/imago-quickstart-imagod.sock"`.
- Confirm `imagod.toml` uses `control_socket_path = "/tmp/imago-quickstart-imagod.sock"`.
- Run `imagod` and `imago service deploy` as the same user, or make sure the socket file is accessible to both.

## Next Steps

- `imago service ls --target default`
- `imago service stop example-service --target default`
