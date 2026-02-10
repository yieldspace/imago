#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
APP_MANIFEST="${ROOT_DIR}/app/Cargo.toml"
WASM_TARGET="wasm32-wasip2"
WASM_BIN_NAME="local-imagod-app"
WASM_SOURCE_DIR="${ROOT_DIR}/app/target/${WASM_TARGET}/release"
WASM_SOURCE="${WASM_SOURCE_DIR}/${WASM_BIN_NAME}.wasm"
WASM_DEST="${ROOT_DIR}/build/app.wasm"
ASSET_PATH="${ROOT_DIR}/assets/message.txt"
MANIFEST_PATH="${ROOT_DIR}/build/manifest.json"

if ! rustup target list --installed | grep -q "^${WASM_TARGET}$"; then
  echo "missing target: ${WASM_TARGET}" >&2
  echo "run: rustup target add ${WASM_TARGET}" >&2
  exit 1
fi

cargo build --manifest-path "${APP_MANIFEST}" --target "${WASM_TARGET}" --release

if [[ ! -f "${WASM_SOURCE}" ]]; then
  alt_name="${WASM_BIN_NAME//-/_}"
  alt_path="${WASM_SOURCE_DIR}/${alt_name}.wasm"
  if [[ -f "${alt_path}" ]]; then
    WASM_SOURCE="${alt_path}"
  else
    echo "wasm output not found: ${WASM_SOURCE}" >&2
    echo "checked fallback path: ${alt_path}" >&2
    exit 1
  fi
fi

mkdir -p "${ROOT_DIR}/build"
cp "${WASM_SOURCE}" "${WASM_DEST}"

wasm_sha="$(shasum -a 256 "${WASM_DEST}" | awk '{print $1}')"
asset_sha="$(shasum -a 256 "${ASSET_PATH}" | awk '{print $1}')"
asset_size="$(wc -c < "${ASSET_PATH}" | tr -d '[:space:]')"

cat > "${MANIFEST_PATH}" <<EOF
{
  "name": "local-imagod-app",
  "main": "build/app.wasm",
  "type": "cli",
  "target": {
    "profile": "local"
  },
  "vars": {
    "IMAGO_EXAMPLE": "local"
  },
  "secrets": {},
  "assets": [
    {
      "path": "assets/message.txt",
      "mount": "/app/message.txt",
      "sha256": "${asset_sha}",
      "size": ${asset_size}
    }
  ],
  "dependencies": [],
  "hash": {
    "algorithm": "sha256",
    "value": "${wasm_sha}",
    "targets": [
      "wasm",
      "manifest",
      "assets"
    ]
  }
}
EOF

echo "generated ${WASM_DEST}"
echo "generated ${MANIFEST_PATH}"
