#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"

cd "${ROOT_DIR}"
OUTPUT="$(cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imago-cli -- logs local-imagod-plugin-hello-app --tail 200)"
printf '%s\n' "${OUTPUT}"

if echo "${OUTPUT}" | rg -qi 'sizumita:ferris|hello from imago|called sizumita:ferris/says.say'; then
  echo "ok: ferris output detected"
  exit 0
fi

echo "error: ferris output not found in logs" >&2
exit 1
