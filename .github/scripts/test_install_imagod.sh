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
mkdir -p "${server_root}/api/tags" "${server_root}/downloads/imagod-v0.6.0" "${server_root}/downloads/imagod-v0.5.0"
cp "${fixture_root}/releases-index.json" "${server_root}/api/releases.json"

write_asset() {
  local release_tag="$1"
  local asset_name="$2"
  local asset_body="$3"
  local asset_dir="${server_root}/downloads/${release_tag}"
  local asset_path="${asset_dir}/${asset_name}"

  mkdir -p "${asset_dir}"
  printf '%s\n' "${asset_body}" > "${asset_path}"
  (
    cd "${asset_dir}"
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

write_asset "imagod-v0.6.0" "imagod-riscv64gc-unknown-linux-musl" '#!/bin/sh
if [ "${1:-}" = "service" ] && [ "${2:-}" = "install" ]; then
  if [ -n "${IMAGOD_TEST_STUB_LOG:-}" ]; then
    printf "%s\n" "$*" >> "${IMAGOD_TEST_STUB_LOG}"
  fi
  exit 0
fi
printf "fixture binary without features\n"
'
write_asset "imagod-v0.6.0" "imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek" '#!/bin/sh
if [ "${1:-}" = "service" ] && [ "${2:-}" = "install" ]; then
  if [ -n "${IMAGOD_TEST_STUB_LOG:-}" ]; then
    printf "%s\n" "$*" >> "${IMAGOD_TEST_STUB_LOG}"
  fi
  exit 0
fi
printf "fixture binary with wasi-nn-cvitek\n"
'
write_asset "imagod-v0.5.0" "imagod-riscv64gc-unknown-linux-musl" '#!/bin/sh
if [ "${1:-}" = "service" ] && [ "${2:-}" = "install" ]; then
  exit 1
fi
printf "legacy fixture binary without service subcommand\n"
'

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
  local release_base_url="${IMAGOD_TEST_RELEASE_BASE_URL:-${download_base}}"
  IMAGOD_RELEASES_API_URL="${base_url}/api/releases.json" \
    IMAGOD_RELEASE_TAG_API_BASE="${base_url}/api/tags" \
    IMAGOD_RELEASE_BASE_URL="${release_base_url}" \
    IMAGOD_TEST_SKIP_PRIVILEGE_ESCALATION="${IMAGOD_TEST_SKIP_PRIVILEGE_ESCALATION:-0}" \
    IMAGOD_TEST_STUB_LOG="${IMAGOD_TEST_STUB_LOG:-}" \
    bash "${repo_root}/scripts/install_imagod.sh" \
      --install-dir "${install_dir}" \
      "$@"
}

run_install_tty() {
  local description="$1"
  local install_dir="$2"
  local transcript_path="$3"
  local tty_input="$4"
  shift 4

  echo "== ${description}"
  local release_base_url="${IMAGOD_TEST_RELEASE_BASE_URL:-${download_base}}"
  IMAGOD_RELEASES_API_URL="${base_url}/api/releases.json" \
    IMAGOD_RELEASE_TAG_API_BASE="${base_url}/api/tags" \
    IMAGOD_RELEASE_BASE_URL="${release_base_url}" \
    IMAGOD_TEST_SKIP_PRIVILEGE_ESCALATION="${IMAGOD_TEST_SKIP_PRIVILEGE_ESCALATION:-0}" \
    IMAGOD_TEST_STUB_LOG="${IMAGOD_TEST_STUB_LOG:-}" \
    INSTALL_IMAGOD_DIR="${install_dir}" \
    INSTALL_IMAGOD_INPUT="${tty_input}" \
    INSTALL_IMAGOD_SCRIPT="${repo_root}/scripts/install_imagod.sh" \
    INSTALL_IMAGOD_TRANSCRIPT="${transcript_path}" \
    python3 - "$@" <<'PY'
import os
import pty
import sys

script = os.environ["INSTALL_IMAGOD_SCRIPT"]
install_dir = os.environ["INSTALL_IMAGOD_DIR"]
transcript_path = os.environ["INSTALL_IMAGOD_TRANSCRIPT"]
tty_input = os.environ.get("INSTALL_IMAGOD_INPUT", "").encode()
cmd = ["bash", script, "--install-dir", install_dir, *sys.argv[1:]]

pid, master_fd = pty.fork()
if pid == 0:
    os.execvpe(cmd[0], cmd, os.environ)

if tty_input:
    os.write(master_fd, tty_input)

chunks = []
while True:
    try:
        chunk = os.read(master_fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    chunks.append(chunk)

os.close(master_fd)
_, status = os.waitpid(pid, 0)
with open(transcript_path, "wb") as transcript_file:
    transcript_file.write(b"".join(chunks))
sys.exit(os.waitstatus_to_exitcode(status))
PY
}

plain_dir="${tmp_root}/plain-install"
feature_dir="${tmp_root}/feature-install"
tty_default_dir="${tmp_root}/tty-default-install"
tty_default_transcript="${tmp_root}/tty-default.transcript"
tty_variant_dir="${tmp_root}/tty-variant-install"
tty_variant_transcript="${tmp_root}/tty-variant.transcript"
service_dir="${tmp_root}/service-install"
service_log="${tmp_root}/service-install.log"
legacy_service_dir="${tmp_root}/legacy-service-install"
legacy_service_log="${tmp_root}/legacy-service.log"
legacy_stub_bin="${tmp_root}/legacy-service-bin"
legacy_systemd_unit="${tmp_root}/legacy-imagod.service"
legacy_download_base="${base_url}/downloads/imagod-v0.5.0"

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

run_install_tty \
  "interactive Enter installs the preselected default variant" \
  "${tty_default_dir}" \
  "${tty_default_transcript}" \
  $'\n' \
  --target riscv64gc-unknown-linux-musl

cmp -s \
  "${tty_default_dir}/imagod" \
  "${server_root}/downloads/imagod-v0.6.0/imagod-riscv64gc-unknown-linux-musl"

grep -F "selected_variant: imagod-riscv64gc-unknown-linux-musl (target=riscv64gc-unknown-linux-musl, features=<none>)" "${tty_default_transcript}"
if grep -F "Available release variants:" "${tty_default_transcript}" >/dev/null 2>&1; then
  echo "unexpected variant list in Enter-to-install transcript" >&2
  exit 1
fi

run_install_tty \
  "interactive s opens the variant list and allows selecting a feature build" \
  "${tty_variant_dir}" \
  "${tty_variant_transcript}" \
  $'s\n2\n' \
  --target riscv64gc-unknown-linux-musl

cmp -s \
  "${tty_variant_dir}/imagod" \
  "${server_root}/downloads/imagod-v0.6.0/imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek"

grep -F "Available release variants:" "${tty_variant_transcript}"
grep -F "[default]" "${tty_variant_transcript}"

IMAGOD_TEST_SKIP_PRIVILEGE_ESCALATION=1 IMAGOD_TEST_STUB_LOG="${service_log}" run_install \
  "install default variant and delegate service setup to imagod service install" \
  "${service_dir}" \
  --target riscv64gc-unknown-linux-musl \
  --with-service

cmp -s \
  "${service_dir}/imagod" \
  "${server_root}/downloads/imagod-v0.6.0/imagod-riscv64gc-unknown-linux-musl"

grep -Fx "service install --config /etc/imago/imagod.toml" "${service_log}"

mkdir -p "${legacy_stub_bin}"
cat > "${legacy_stub_bin}/install" <<'EOF'
#!/bin/sh
set -eu

mode=""
create_dir=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    -m)
      mode="$2"
      shift 2
      ;;
    -d)
      create_dir=1
      shift
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
  esac
done

if [ "${create_dir}" = "1" ]; then
  mkdir -p -- "$@"
  exit 0
fi

src="$1"
dest="$2"
if [ "${dest}" = "/etc/systemd/system/imagod.service" ]; then
  dest="${IMAGOD_TEST_SYSTEMD_UNIT_PATH}"
fi
mkdir -p -- "$(dirname "${dest}")"
cp "${src}" "${dest}"
if [ -n "${mode}" ]; then
  chmod "${mode}" "${dest}"
fi
EOF
chmod +x "${legacy_stub_bin}/install"

cat > "${legacy_stub_bin}/systemctl" <<'EOF'
#!/bin/sh
set -eu
printf "%s\n" "$*" >> "${IMAGOD_TEST_STUB_LOG}"
EOF
chmod +x "${legacy_stub_bin}/systemctl"

PATH="${legacy_stub_bin}:${PATH}" \
IMAGOD_TEST_SERVICE_MANAGER=systemd \
IMAGOD_TEST_SYSTEMD_UNIT_PATH="${legacy_systemd_unit}" \
IMAGOD_TEST_SKIP_PRIVILEGE_ESCALATION=1 \
IMAGOD_TEST_STUB_LOG="${legacy_service_log}" \
IMAGOD_TEST_RELEASE_BASE_URL="${legacy_download_base}" \
run_install \
  "install older tagged release and fall back to legacy service setup" \
  "${legacy_service_dir}" \
  --target riscv64gc-unknown-linux-musl \
  --tag imagod-v0.5.0 \
  --with-service

cmp -s \
  "${legacy_service_dir}/imagod" \
  "${server_root}/downloads/imagod-v0.5.0/imagod-riscv64gc-unknown-linux-musl"

grep -Fx "daemon-reload" "${legacy_service_log}"
grep -Fx "enable --now imagod.service" "${legacy_service_log}"
grep -F "ExecStart=${legacy_service_dir}/imagod --config /etc/imago/imagod.toml" "${legacy_systemd_unit}"
