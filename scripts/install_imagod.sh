#!/usr/bin/env bash
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"

DEFAULT_REPO="yieldspace/imago"
DEFAULT_CONFIG_PATH="/etc/imago/imagod.toml"

REPO="${DEFAULT_REPO}"
TAG_INPUT=""
LIBC_MODE="auto"
INSTALL_DIR=""
NO_SERVICE=0
DRY_RUN=0

log() {
  printf '[%s] %s\n' "${SCRIPT_NAME}" "$*"
}

warn() {
  printf '[%s] warning: %s\n' "${SCRIPT_NAME}" "$*" >&2
}

die() {
  printf '[%s] error: %s\n' "${SCRIPT_NAME}" "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: install_imagod.sh [options]

Options:
  --tag <semver|imagod-vX.Y.Z>      Install a specific release tag/version.
  --libc <auto|gnu|musl>            libc selection (default: auto).
  --repo <owner/repo>               GitHub repository (default: yieldspace/imago).
  --install-dir <path>              Binary install directory.
  --no-service                      Skip service setup.
  --dry-run                         Print resolved values without downloading/installing.
  -h, --help                        Show this help.

Environment:
  GH_TOKEN                          Optional GitHub token for API/download requests.
  GITHUB_REF_NAME                   If imagod-v* and --tag is omitted, this tag is used.
  IMAGOD_RELEASE_BASE_URL           Optional test override for release asset base URL.

Notes:
  - Linux only.
  - Service setup priority: systemd -> init.d -> binary-only.
  - Use --libc to override auto detection if your environment is atypical.
EOF
}

need_cmd() {
  local cmd="$1"
  command -v "${cmd}" >/dev/null 2>&1 || die "required command not found: ${cmd}"
}

normalize_tag() {
  local input="$1"
  if [[ "${input}" =~ ^imagod-v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]]; then
    printf '%s\n' "${input}"
    return 0
  fi

  if [[ "${input}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]]; then
    printf 'imagod-v%s\n' "${input}"
    return 0
  fi

  die "invalid --tag value: ${input}"
}

build_curl_headers() {
  CURL_HEADERS=(
    -H "Accept: application/vnd.github+json"
    -H "X-GitHub-Api-Version: 2022-11-28"
  )
  if [[ -n "${GH_TOKEN:-}" ]]; then
    CURL_HEADERS+=(-H "Authorization: Bearer ${GH_TOKEN}")
  fi
}

normalize_libc_mode() {
  local input="$1"
  case "${input}" in
    auto | gnu | musl)
      printf '%s\n' "${input}"
      ;;
    *)
      die "invalid --libc value: ${input} (expected: auto|gnu|musl)"
      ;;
  esac
}

resolve_latest_imagod_tag() {
  build_curl_headers
  local per_page=100
  local max_pages=20
  local page=1

  while [[ "${page}" -le "${max_pages}" ]]; do
    local api_url="https://api.github.com/repos/${REPO}/releases?per_page=${per_page}&page=${page}"
    local body
    if ! body="$(curl -fsSL "${CURL_HEADERS[@]}" "${api_url}")"; then
      if [[ -z "${GH_TOKEN:-}" ]]; then
        die "failed to fetch releases from ${api_url} (repository may be private; set GH_TOKEN)"
      fi
      die "failed to fetch releases from ${api_url}"
    fi

    local tag
    tag="$(
      printf '%s' "${body}" \
        | tr -d '\n' \
        | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"imagod-v[^"]*"' \
        | head -n1 \
        | sed -E 's/"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)"/\1/' \
        || true
    )"
    if [[ -n "${tag}" ]]; then
      printf '%s\n' "${tag}"
      return 0
    fi

    local release_count
    release_count="$(printf '%s' "${body}" | grep -o '"tag_name"[[:space:]]*:' | wc -l | tr -d ' ')"
    if [[ "${release_count}" -lt "${per_page}" ]]; then
      break
    fi

    page=$((page + 1))
  done

  die "no imagod-v* release tag found in ${REPO}; specify --tag explicitly if needed"
}

resolve_release_tag() {
  if [[ -n "${TAG_INPUT}" ]]; then
    normalize_tag "${TAG_INPUT}"
    return 0
  fi

  if [[ "${GITHUB_REF_NAME:-}" =~ ^imagod-v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]]; then
    printf '%s\n' "${GITHUB_REF_NAME}"
    return 0
  fi

  resolve_latest_imagod_tag
}

detect_linux() {
  local uname_s="${IMAGOD_TEST_UNAME_S:-$(uname -s)}"
  [[ "${uname_s}" == "Linux" ]] || die "unsupported OS: ${uname_s} (Linux only)"
}

detect_arch() {
  local uname_m="${IMAGOD_TEST_UNAME_M:-$(uname -m)}"
  case "${uname_m}" in
    x86_64 | amd64) printf 'x86_64\n' ;;
    aarch64 | arm64) printf 'aarch64\n' ;;
    armv7l | armv7hf | armv7) printf 'armv7\n' ;;
    riscv64 | riscv64gc) printf 'riscv64gc\n' ;;
    *)
      die "unsupported architecture: ${uname_m}"
      ;;
  esac
}

detect_libc() {
  local arch="$1"
  if [[ "${LIBC_MODE}" != "auto" ]]; then
    printf '%s\n' "${LIBC_MODE}"
    return 0
  fi

  if [[ -n "${IMAGOD_TEST_LIBC:-}" ]]; then
    case "${IMAGOD_TEST_LIBC}" in
      gnu | musl)
        printf '%s\n' "${IMAGOD_TEST_LIBC}"
        return 0
        ;;
      *)
        die "IMAGOD_TEST_LIBC must be 'gnu' or 'musl'"
        ;;
    esac
  fi

  if command -v ldd >/dev/null 2>&1; then
    local ldd_out
    ldd_out="$(ldd --version 2>&1 || true)"
    if printf '%s' "${ldd_out}" | grep -qi 'musl'; then
      printf 'musl\n'
      return 0
    fi
    if printf '%s' "${ldd_out}" | grep -qiE '(glibc|gnu libc)'; then
      printf 'gnu\n'
      return 0
    fi
  fi

  if command -v getconf >/dev/null 2>&1 && getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
    printf 'gnu\n'
    return 0
  fi

  if compgen -G "/lib/ld-musl-*.so.*" >/dev/null || compgen -G "/lib64/ld-musl-*.so.*" >/dev/null || compgen -G "/usr/lib/ld-musl-*.so.*" >/dev/null; then
    warn "libc auto-detection inferred 'musl' from dynamic loader files; rerun with --libc gnu|musl to override"
    printf 'musl\n'
    return 0
  fi

  if compgen -G "/lib*/ld-linux*.so*" >/dev/null || compgen -G "/usr/lib*/ld-linux*.so*" >/dev/null; then
    warn "libc auto-detection inferred 'gnu' from dynamic loader files; rerun with --libc gnu|musl to override"
    printf 'gnu\n'
    return 0
  fi

  case "${arch}" in
    aarch64 | armv7 | riscv64gc)
      warn "libc auto-detection is inconclusive; falling back to 'musl' for arch=${arch}. rerun with --libc gnu|musl if needed"
      printf 'musl\n'
      ;;
    x86_64)
      warn "libc auto-detection is inconclusive; falling back to 'gnu' for arch=${arch}. rerun with --libc gnu|musl if needed"
      printf 'gnu\n'
      ;;
    *)
      die "libc auto-detection failed for arch=${arch}; rerun with --libc gnu|musl"
      ;;
  esac
}

resolve_target_triple() {
  local arch="$1"
  local libc="$2"

  case "${arch}:${libc}" in
    x86_64:gnu) printf 'x86_64-unknown-linux-gnu\n' ;;
    x86_64:musl) printf 'x86_64-unknown-linux-musl\n' ;;
    aarch64:gnu) printf 'aarch64-unknown-linux-gnu\n' ;;
    armv7:gnu) printf 'armv7-unknown-linux-gnueabihf\n' ;;
    riscv64gc:gnu) printf 'riscv64gc-unknown-linux-gnu\n' ;;
    aarch64:musl) printf 'aarch64-unknown-linux-musl\n' ;;
    armv7:musl) printf 'armv7-unknown-linux-musleabihf\n' ;;
    riscv64gc:musl) printf 'riscv64gc-unknown-linux-musl\n' ;;
    *)
      die "unsupported target combination: arch=${arch}, libc=${libc}"
      ;;
  esac
}

default_install_dir() {
  if [[ "$(id -u)" -eq 0 ]]; then
    printf '/usr/local/bin\n'
  else
    printf '%s/.local/bin\n' "${HOME}"
  fi
}

run_as_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
    return 0
  fi

  if command -v sudo >/dev/null 2>&1; then
    sudo "$@"
    return 0
  fi

  return 1
}

validate_service_binary_path() {
  local path="$1"

  if [[ "${path}" != /* ]]; then
    warn "service setup skipped because binary path is not absolute: ${path}"
    warn "use an absolute --install-dir or run with --no-service"
    return 1
  fi

  if [[ ! "${path}" =~ ^/[A-Za-z0-9._/+:-]+$ ]]; then
    warn "service setup skipped because binary path contains unsafe characters: ${path}"
    warn "allowed characters: A-Z a-z 0-9 . _ / + : -"
    return 1
  fi

  return 0
}

validate_install_dir_path() {
  local path="$1"
  if [[ ! "${path}" =~ ^[A-Za-z0-9._/+:-]+$ ]]; then
    die "invalid --install-dir: '${path}' (contains unsafe characters)"
  fi
}

download_release_asset() {
  local url="$1"
  local output="$2"
  local args=(--fail --silent --show-error --location --output "${output}")

  if [[ -n "${GH_TOKEN:-}" ]]; then
    args+=(--header "Authorization: Bearer ${GH_TOKEN}")
  fi

  curl "${args[@]}" "${url}" || die "failed to download: ${url}"
}

install_binary() {
  local source_bin="$1"
  local destination_dir="$2"
  local destination_bin="${destination_dir}/imagod"

  if mkdir -p "${destination_dir}" 2>/dev/null && install -m 0755 "${source_bin}" "${destination_bin}" 2>/dev/null; then
    printf '%s\n' "${destination_bin}"
    return 0
  fi

  run_as_root install -d "${destination_dir}" || die "failed to create install dir: ${destination_dir}"
  run_as_root install -m 0755 "${source_bin}" "${destination_bin}" || die "failed to install imagod to ${destination_bin}"
  printf '%s\n' "${destination_bin}"
}

setup_systemd_service() {
  local binary_path="$1"
  local unit_tmp
  unit_tmp="$(mktemp)"
  cat > "${unit_tmp}" <<EOF
[Unit]
Description=imagod daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${binary_path} --config ${DEFAULT_CONFIG_PATH}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

  if ! run_as_root install -m 0644 "${unit_tmp}" /etc/systemd/system/imagod.service; then
    rm -f "${unit_tmp}"
    warn "failed to install systemd unit; keeping binary-only installation"
    return 1
  fi
  rm -f "${unit_tmp}"

  if ! run_as_root systemctl daemon-reload; then
    warn "systemctl daemon-reload failed; keeping binary-only installation"
    return 1
  fi

  if ! run_as_root systemctl enable --now imagod.service; then
    warn "systemctl enable/start failed; keeping binary-only installation"
    return 1
  fi

  log "systemd service installed and started: imagod.service"
  return 0
}

setup_initd_service() {
  local binary_path="$1"
  local init_tmp
  init_tmp="$(mktemp)"

  cat > "${init_tmp}" <<EOF
#!/bin/sh
### BEGIN INIT INFO
# Provides:          imagod
# Required-Start:    \$remote_fs \$network
# Required-Stop:     \$remote_fs \$network
# Default-Start:     2 3 4 5
# Default-Stop:      0 1 6
# Short-Description: imagod daemon
### END INIT INFO

DAEMON='${binary_path}'
DAEMON_ARGS='--config ${DEFAULT_CONFIG_PATH}'
PIDFILE="/var/run/imagod.pid"
NAME="imagod"

start() {
  if command -v start-stop-daemon >/dev/null 2>&1; then
    start-stop-daemon --start --quiet --background --make-pidfile --pidfile "\$PIDFILE" --exec "\$DAEMON" -- \$DAEMON_ARGS
    return \$?
  fi

  "\$DAEMON" \$DAEMON_ARGS >/dev/null 2>&1 &
  echo \$! > "\$PIDFILE"
}

stop() {
  if command -v start-stop-daemon >/dev/null 2>&1; then
    start-stop-daemon --stop --quiet --pidfile "\$PIDFILE" --retry 5
    rm -f "\$PIDFILE"
    return \$?
  fi

  if [ -f "\$PIDFILE" ]; then
    kill "\$(cat "\$PIDFILE")" >/dev/null 2>&1 || true
    rm -f "\$PIDFILE"
  fi
}

status() {
  if [ -f "\$PIDFILE" ] && kill -0 "\$(cat "\$PIDFILE")" >/dev/null 2>&1; then
    echo "\$NAME is running"
    exit 0
  fi
  echo "\$NAME is not running"
  exit 3
}

case "\$1" in
  start) start ;;
  stop) stop ;;
  restart) stop; start ;;
  status) status ;;
  *) echo "Usage: /etc/init.d/\$NAME {start|stop|restart|status}"; exit 1 ;;
esac
EOF

  if ! run_as_root install -m 0755 "${init_tmp}" /etc/init.d/imagod; then
    rm -f "${init_tmp}"
    warn "failed to install init.d script; keeping binary-only installation"
    return 1
  fi
  rm -f "${init_tmp}"

  if command -v update-rc.d >/dev/null 2>&1; then
    run_as_root update-rc.d imagod defaults || warn "update-rc.d failed"
  elif command -v chkconfig >/dev/null 2>&1; then
    run_as_root chkconfig --add imagod || warn "chkconfig --add failed"
  else
    warn "no init.d enable command found (update-rc.d/chkconfig); autostart not configured"
  fi

  if command -v service >/dev/null 2>&1; then
    run_as_root service imagod start || warn "failed to start imagod via service command"
  else
    run_as_root /etc/init.d/imagod start || warn "failed to start /etc/init.d/imagod"
  fi

  log "init.d service installed (start attempted)"
  return 0
}

maybe_setup_service() {
  local binary_path="$1"
  if [[ "${NO_SERVICE}" -eq 1 ]]; then
    log "--no-service is set; skipped service setup"
    return 0
  fi

  if ! validate_service_binary_path "${binary_path}"; then
    return 0
  fi

  if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
    setup_systemd_service "${binary_path}" || true
    return 0
  fi

  if [[ -d /etc/init.d ]]; then
    setup_initd_service "${binary_path}" || true
    return 0
  fi

  warn "no supported init system detected; binary installed only"
}

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --tag)
        [[ $# -ge 2 ]] || die "--tag requires a value"
        TAG_INPUT="$2"
        shift 2
        ;;
      --libc)
        [[ $# -ge 2 ]] || die "--libc requires a value"
        LIBC_MODE="$(normalize_libc_mode "$2")"
        shift 2
        ;;
      --repo)
        [[ $# -ge 2 ]] || die "--repo requires a value"
        REPO="$2"
        shift 2
        ;;
      --install-dir)
        [[ $# -ge 2 ]] || die "--install-dir requires a value"
        INSTALL_DIR="$2"
        shift 2
        ;;
      --no-service)
        NO_SERVICE=1
        shift
        ;;
      --dry-run)
        DRY_RUN=1
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        die "unknown argument: $1"
        ;;
    esac
  done
}

main() {
  parse_args "$@"

  need_cmd curl
  need_cmd sha256sum
  need_cmd install

  detect_linux

  local arch
  local libc
  local target
  local tag

  arch="$(detect_arch)"
  libc="$(detect_libc "${arch}")"
  target="$(resolve_target_triple "${arch}" "${libc}")"
  tag="$(resolve_release_tag)"

  local asset_name="imagod-${target}"
  local checksum_name="${asset_name}.sha256"
  local release_url_base_default="https://github.com/${REPO}/releases/download/${tag}"
  local release_url_base="${IMAGOD_RELEASE_BASE_URL:-${release_url_base_default}}"
  local install_dir
  if [[ -n "${INSTALL_DIR}" ]]; then
    install_dir="${INSTALL_DIR}"
  else
    install_dir="$(default_install_dir)"
  fi
  validate_install_dir_path "${install_dir}"

  log "repository: ${REPO}"
  log "tag: ${tag}"
  log "target: ${target}"
  log "install_dir: ${install_dir}"

  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "dry-run enabled; no changes applied"
    log "binary URL: ${release_url_base}/${asset_name}"
    log "checksum URL: ${release_url_base}/${checksum_name}"
    exit 0
  fi

  local tmp_dir
  tmp_dir="$(mktemp -d)"
  trap "rm -rf '${tmp_dir}'" EXIT

  download_release_asset "${release_url_base}/${asset_name}" "${tmp_dir}/${asset_name}"
  download_release_asset "${release_url_base}/${checksum_name}" "${tmp_dir}/${checksum_name}"

  (
    cd "${tmp_dir}"
    sha256sum -c "${checksum_name}"
  ) || die "checksum verification failed for ${asset_name}"

  local installed_bin
  installed_bin="$(install_binary "${tmp_dir}/${asset_name}" "${install_dir}")"
  log "installed binary: ${installed_bin}"

  maybe_setup_service "${installed_bin}"

  log "imagod installation completed"
  log "run '${installed_bin} --help' to verify installation"
}

main "$@"
