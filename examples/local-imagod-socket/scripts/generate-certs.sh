#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"
CERTS_DIR="${ROOT_DIR}/certs"

cd "${ROOT_DIR}"
cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imago-cli -- \
  certs generate \
  --out-dir "${CERTS_DIR}" \
  --force

cat > "${CERTS_DIR}/.gitignore" <<'GITIGNORE'
*
!.gitignore
!server.key
!client.key
!server.pub.hex
!client.pub.hex
GITIGNORE

CLIENT_PUB_HEX="$(tr -d '[:space:]' < "${CERTS_DIR}/client.pub.hex")"
if [[ "${#CLIENT_PUB_HEX}" -ne 64 ]] || [[ ! "${CLIENT_PUB_HEX}" =~ ^[0-9a-fA-F]{64}$ ]]; then
  echo "invalid ed25519 raw hex in ${CERTS_DIR}/client.pub.hex; expected 64 hex chars" >&2
  exit 1
fi

perl -0pi -e "s/client_public_keys = \\\[\\\"[^\\\"]+\\\"\\\]/client_public_keys = [\\\"${CLIENT_PUB_HEX}\\\"]/g" "${ROOT_DIR}/imagod.toml"

echo "updated ${ROOT_DIR}/imagod.toml tls.client_public_keys"
echo "known_hosts is managed by CLI at ~/.imago/known_hosts"
