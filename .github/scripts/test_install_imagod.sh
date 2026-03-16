#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture_root="${repo_root}/.github/testdata/install_imagod"
tmp_root="$(mktemp -d)"
server_pid=""

cleanup() {
  if [[ -n "${server_pid}" ]]; then
    kill "${server_pid}" >/dev/null 2>&1 || true
    wait "${server_pid}" 2>/dev/null || true
  fi
  rm -rf "${tmp_root}"
}

trap cleanup EXIT HUP INT TERM

server_root="${tmp_root}/server"
mkdir -p "${server_root}/api/tags" "${server_root}/downloads/imagod-v0.6.0"
cp "${fixture_root}/releases-index.json" "${server_root}/api/releases.json"

write_asset() {
  local asset_name="$1"
  local asset_body="$2"
  local asset_path="${server_root}/downloads/imagod-v0.6.0/${asset_name}"

  printf '%s\n' "${asset_body}" > "${asset_path}"
  (
    cd "${server_root}/downloads/imagod-v0.6.0"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "${asset_name}" > "${asset_name}.sha256"
    elif command -v shasum >/dev/null 2>&1; then
      shasum -a 256 "${asset_name}" > "${asset_name}.sha256"
    else
      echo "required checksum command not found: sha256sum or shasum" >&2
      exit 1
    fi
  )
}

write_asset "imagod-riscv64gc-unknown-linux-musl" "fixture binary without features"
write_asset "imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek" "fixture binary with wasi-nn-cvitek"

port_file="${tmp_root}/http.port"
python3 - <<'PY' "${server_root}" "${port_file}" &
import contextlib
import functools
import http.server
import pathlib
import socketserver
import sys

root = pathlib.Path(sys.argv[1])
port_file = pathlib.Path(sys.argv[2])
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(root))

with socketserver.TCPServer(("127.0.0.1", 0), handler) as httpd:
    port_file.write_text(str(httpd.server_address[1]), encoding="utf-8")
    httpd.serve_forever()
PY
server_pid=$!

for _ in $(seq 1 50); do
  if [[ -s "${port_file}" ]]; then
    break
  fi
  sleep 0.1
done

if [[ ! -s "${port_file}" ]]; then
  echo "failed to start local fixture server" >&2
  exit 1
fi

port="$(cat "${port_file}")"
base_url="http://127.0.0.1:${port}"
download_base="${base_url}/downloads/imagod-v0.6.0"
sed "s#__DOWNLOAD_BASE__#${download_base}#g" \
  "${fixture_root}/release-imagod-v0.6.0.json.tmpl" \
  > "${server_root}/api/tags/imagod-v0.6.0"

run_install() {
  local description="$1"
  local install_dir="$2"
  shift 2

  echo "== ${description}"
  IMAGOD_RELEASES_API_URL="${base_url}/api/releases.json" \
    IMAGOD_RELEASE_TAG_API_BASE="${base_url}/api/tags" \
    IMAGOD_RELEASE_BASE_URL="${download_base}" \
    bash "${repo_root}/scripts/install_imagod.sh" \
      --install-dir "${install_dir}" \
      "$@"
}

plain_dir="${tmp_root}/plain-install"
feature_dir="${tmp_root}/feature-install"

run_install \
  "install default variant from fixture catalog" \
  "${plain_dir}" \
  --target riscv64gc-unknown-linux-musl

cmp -s \
  "${plain_dir}/imagod" \
  "${server_root}/downloads/imagod-v0.6.0/imagod-riscv64gc-unknown-linux-musl"

run_install \
  "install feature variant from fixture catalog" \
  "${feature_dir}" \
  --target riscv64gc-unknown-linux-musl \
  --features wasi-nn-cvitek

cmp -s \
  "${feature_dir}/imagod" \
  "${server_root}/downloads/imagod-v0.6.0/imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek"
