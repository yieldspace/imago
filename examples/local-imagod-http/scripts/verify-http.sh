#!/usr/bin/env bash
set -euo pipefail

PORT="${1:-18080}"
BODY="$(curl -fsS "http://127.0.0.1:${PORT}/")"

if [[ "${BODY}" != "hello from local-imagod-http" ]]; then
  echo "unexpected response body: ${BODY}" >&2
  exit 1
fi

echo "ok: ${BODY}"
