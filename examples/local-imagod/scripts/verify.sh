#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../_shared" && pwd)/common.sh"

resolve_paths "${BASH_SOURCE[0]}"

TAIL_LINES="${1:-200}"
if ! [[ "${TAIL_LINES}" =~ ^[0-9]+$ ]] || [[ "${TAIL_LINES}" -le 0 ]]; then
  echo "error: --tail value must be a positive integer: ${TAIL_LINES}" >&2
  exit 1
fi

if ! OUTPUT="$(run_imago_cli logs local-imagod-app --tail "${TAIL_LINES}" 2>&1)"; then
  printf '%s\n' "${OUTPUT}" >&2
  echo "error: failed to fetch logs for local-imagod-app" >&2
  exit 1
fi

printf '%s\n' "${OUTPUT}"

if ! printf '%s\n' "${OUTPUT}" | rg -q 'local-imagod-app started'; then
  echo "error: expected log line not found: local-imagod-app started" >&2
  echo "hint: ensure imagod is running and deploy completed successfully" >&2
  exit 1
fi

echo "ok: local-imagod-app started log detected"
