#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"

cd "${ROOT_DIR}"
cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imagod -- --config "${ROOT_DIR}/imagod.toml"

