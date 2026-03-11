# imago-with-componentize-py example

## Purpose

This example demonstrates deploying a Python app as a Wasm Component with componentize-py and `imago service deploy`.

## Prerequisites

- Python and `uv`
- Rust toolchain for running `imago-cli` and `imagod`
- OpenSSH client/server, with `ssh localhost true` succeeding non-interactively

`imago service deploy` now reaches local `imagod` via `ssh://localhost?...` and `imagod proxy-stdio`.
Make sure the SSH login shell can execute `imagod` from `PATH`.

Install the Python dev dependencies:

```bash
cd examples/imago-with-componentize-py
uv sync --group dev
```

## Generate Bindings

`main.py` imports `wasi.wit_world`, so generate the WASI bindings first:

```bash
cd examples/imago-with-componentize-py
uv sync --group dev
uv run componentize-py -d wit -w exports bindings wasi
```

This command generates Python bindings under `wasi/` (including `wasi/wit_world/...`).

## Build Wasm Component

Build `cli.wasm` manually with the same command configured in `[build].command` in `imago.toml`:

```bash
cd examples/imago-with-componentize-py
uv run componentize-py -d wit -w wasi:cli/command@0.2.0 componentize main -o cli.wasm
```

`imago service deploy` also uses this command when it performs a build.

## Run

1. Start `imagod` in terminal A.

```bash
cd examples/imago-with-componentize-py
cargo run -p imagod -- --config imagod.toml
```

2. Deploy and check logs in terminal B.

```bash
cd examples/imago-with-componentize-py
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs imago-with-python --tail 200
```

## Success Criteria

Deployment is successful when service logs include `Hello, world from python!`.

## Troubleshooting

### `runner_ready` timeout during `service deploy`

If deploy fails with `did not send runner_ready in time`:

1. Confirm binding generation and component build both succeed:

```bash
cd examples/imago-with-componentize-py
uv run componentize-py -d wit -w exports bindings wasi
uv run componentize-py -d wit -w wasi:cli/command@0.2.0 componentize main -o cli.wasm
```

2. Check `imagod` logs (terminal A) for errors that occur before the runner reaches ready state.
3. Increase `runner_ready_timeout_secs` in `imagod.toml` and retry. This helps separate slow startup from a hard failure.

### SSH localhost errors during `service deploy`

Check these points before retrying:

1. `ssh localhost true` succeeds without an interactive prompt.
2. `imagod proxy-stdio --socket /tmp/imagod-componentize-py.sock` is available from the SSH login shell.
3. `imago.toml` and `imagod.toml` agree on the socket path.
