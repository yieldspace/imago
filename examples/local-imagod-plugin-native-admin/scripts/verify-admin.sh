#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"

cd "${ROOT_DIR}"
OUTPUT="$(cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imago-cli -- logs local-imagod-plugin-native-admin-app --tail 200)"
printf '%s\n' "${OUTPUT}"

if ! echo "${OUTPUT}" | rg -q 'imago-admin service-name='; then
  echo "error: service-name output not found" >&2
  exit 1
fi
if ! echo "${OUTPUT}" | rg -q 'imago-admin release-hash='; then
  echo "error: release-hash output not found" >&2
  exit 1
fi
if ! echo "${OUTPUT}" | rg -q 'imago-admin runner-id='; then
  echo "error: runner-id output not found" >&2
  exit 1
fi
if ! echo "${OUTPUT}" | rg -q 'imago-admin app-type=cli'; then
  echo "error: app-type output not found" >&2
  exit 1
fi

echo "ok: imago:admin native plugin output detected"
