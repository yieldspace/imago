#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"

cd "${ROOT_DIR}"
cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imago-cli -- \
  certs generate \
  --out-dir "${ROOT_DIR}/certs" \
  --server-name localhost \
  --server-ip 127.0.0.1 \
  --days 3650 \
  --force
