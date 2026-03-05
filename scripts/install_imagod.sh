#!/usr/bin/env bash
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"

DEFAULT_REPO="yieldspace/imago"
DEFAULT_CONFIG_PATH="/etc/imago/imagod.toml"
LAUNCHD_PLIST_PATH="/Library/LaunchDaemons/imagod.plist"

REPO="${DEFAULT_REPO}"
TAG_INPUT=""
TARGET_OVERRIDE=""
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
  cat <<'USAGE'
Usage: install_imagod.sh [options]

Options:
  --tag <semver|imagod-vX.Y.Z>      Install a specific release tag/version.
  --target <triple>                 Override auto-detected target triple.
  --repo <owner/repo>               GitHub repository (default: yieldspace/imago).
  --install-dir <path>              Binary install directory.
  --no-service                      Skip service setup.
  --dry-run                         Print resolved values without downloading/installing.
  -h, --help                        Show this help.

Environment:
  GH_TOKEN                          Optional GitHub token for download requests.
  IMAGOD_RELEASE_BASE_URL           Optional test override for release asset base URL.

Notes:
  - Supported OS: Linux and macOS.
  - Tag resolution: --tag > latest imagod-v* tag from git refs.
  - Target resolution: --target > auto-detect.
  - Service setup priority:
      Linux: systemd -> init.d -> binary-only
      macOS: launchd(system daemon) -> binary-only
USAGE
}

check_cmd() {
  command -v "$1" >/dev/null 2>&1
}

need_cmd() {
  local cmd="$1"
  check_cmd "${cmd}" || die "required command not found: ${cmd}"
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

validate_target_override() {
  local target="$1"

  if [[ -z "${target}" ]]; then
    die "--target requires a non-empty value"
  fi

  if [[ "${target}" =~ [[:space:]] ]]; then
    die "invalid --target value: '${target}' (whitespace is not allowed)"
  fi

  if [[ "${target}" == *,* ]]; then
    die "invalid --target value: '${target}' (comma-separated values are not supported; pass a single triple)"
  fi

  if [[ ! "${target}" =~ ^[A-Za-z0-9._-]+$ ]]; then
    die "invalid --target value: '${target}' (allowed characters: A-Z a-z 0-9 . _ -)"
  fi

  printf '%s\n' "${target}"
}

resolve_release_tag() {
  if [[ -n "${TAG_INPUT}" ]]; then
    normalize_tag "${TAG_INPUT}"
    return 0
  fi

  printf ''
}

resolve_latest_imagod_tag_from_git_refs() {
  check_cmd git || die "git command is required when --tag is omitted; install git or pass --tag imagod-vX.Y.Z"

  local remote_url="https://github.com/${REPO}.git"
  local refs
  if ! refs="$(git ls-remote --refs --tags --sort='v:refname' "${remote_url}" 'imagod-v*' 2>/dev/null)"; then
    die "failed to query imagod tags from ${remote_url}; pass --tag imagod-vX.Y.Z explicitly"
  fi

  local latest_tag
  latest_tag="$(printf '%s\n' "${refs}" | awk -F/ 'NF {print $NF}' | tail -n1)"
  if [[ -z "${latest_tag}" ]]; then
    die "no imagod-v* tags found in ${REPO}; pass --tag imagod-vX.Y.Z explicitly"
  fi

  printf '%s\n' "${latest_tag}"
}

resolve_release_url_base() {
  local tag="$1"
  if [[ -n "${IMAGOD_RELEASE_BASE_URL:-}" ]]; then
    printf '%s\n' "${IMAGOD_RELEASE_BASE_URL}"
    return 0
  fi

  if [[ -n "${tag}" ]]; then
    printf 'https://github.com/%s/releases/download/%s\n' "${REPO}" "${tag}"
    return 0
  fi

  printf 'https://github.com/%s/releases/latest/download\n' "${REPO}"
}

detect_os() {
  local uname_s="${IMAGOD_TEST_UNAME_S:-$(uname -s)}"
  case "${uname_s}" in
    Linux) printf 'linux\n' ;;
    Darwin) printf 'darwin\n' ;;
    *)
      die "unsupported OS: ${uname_s} (supported: Linux, Darwin)"
      ;;
  esac
}

darwin_sysctl_has_one() {
  local key="$1"
  local override_var="$2"
  local forced="${!override_var-}"

  if [[ -n "${forced}" ]]; then
    [[ "${forced}" == "1" ]]
    return $?
  fi

  if ! check_cmd sysctl; then
    return 1
  fi

  (sysctl "${key}" 2>/dev/null || true) | grep -q ': 1'
}

detect_darwin_arch() {
  local uname_m="${IMAGOD_TEST_UNAME_M:-$(uname -m)}"
  case "${uname_m}" in
    i386)
      # macOS on Intel may report i386 for compatibility shells.
      if darwin_sysctl_has_one "hw.optional.x86_64" "IMAGOD_TEST_SYSCTL_HW_OPTIONAL_X86_64"; then
        uname_m="x86_64"
      fi
      ;;
    x86_64 | amd64)
      # Under Rosetta 2, uname may report x86_64 on arm64 hardware.
      if darwin_sysctl_has_one "hw.optional.arm64" "IMAGOD_TEST_SYSCTL_HW_OPTIONAL_ARM64"; then
        uname_m="arm64"
      fi
      ;;
  esac

  case "${uname_m}" in
    x86_64 | amd64) printf 'x86_64\n' ;;
    arm64 | aarch64) printf 'aarch64\n' ;;
    *)
      die "unsupported architecture on macOS: ${uname_m}"
      ;;
  esac
}

detect_linux_arch() {
  local uname_m="${IMAGOD_TEST_UNAME_M:-$(uname -m)}"
  case "${uname_m}" in
    x86_64 | amd64) printf 'x86_64\n' ;;
    aarch64 | arm64) printf 'aarch64\n' ;;
    armv7l | armv7hf | armv7) printf 'armv7\n' ;;
    riscv64 | riscv64gc) printf 'riscv64gc\n' ;;
    *)
      die "unsupported architecture on Linux: ${uname_m}"
      ;;
  esac
}

detect_arch() {
  local os="$1"
  case "${os}" in
    linux) detect_linux_arch ;;
    darwin) detect_darwin_arch ;;
    *)
      die "unsupported os in detect_arch: ${os}"
      ;;
  esac
}

detect_linux_libc() {
  local arch="$1"

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

  if check_cmd ldd; then
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

  if check_cmd getconf && getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
    printf 'gnu\n'
    return 0
  fi

  if compgen -G "/lib/ld-musl-*.so.*" >/dev/null || compgen -G "/lib64/ld-musl-*.so.*" >/dev/null || compgen -G "/usr/lib/ld-musl-*.so.*" >/dev/null; then
    warn "libc auto-detection inferred 'musl' from dynamic loader files; rerun with --target <triple> to override"
    printf 'musl\n'
    return 0
  fi

  if compgen -G "/lib*/ld-linux*.so*" >/dev/null || compgen -G "/usr/lib*/ld-linux*.so*" >/dev/null; then
    warn "libc auto-detection inferred 'gnu' from dynamic loader files; rerun with --target <triple> to override"
    printf 'gnu\n'
    return 0
  fi

  case "${arch}" in
    aarch64 | armv7 | riscv64gc)
      warn "libc auto-detection is inconclusive; falling back to 'musl' for arch=${arch}. rerun with --target <triple> if needed"
      printf 'musl\n'
      ;;
    x86_64)
      warn "libc auto-detection is inconclusive; falling back to 'gnu' for arch=${arch}. rerun with --target <triple> if needed"
      printf 'gnu\n'
      ;;
    *)
      die "libc auto-detection failed for arch=${arch}; rerun with --target <triple>"
      ;;
  esac
}

detect_libc() {
  local os="$1"
  local arch="$2"

  if [[ "${os}" != "linux" ]]; then
    printf 'none\n'
    return 0
  fi

  detect_linux_libc "${arch}"
}

resolve_target_triple() {
  local os="$1"
  local arch="$2"
  local libc="$3"

  case "${os}" in
    darwin)
      case "${arch}" in
        x86_64) printf 'x86_64-apple-darwin\n' ;;
        aarch64) printf 'aarch64-apple-darwin\n' ;;
        *)
          die "unsupported target combination: os=${os}, arch=${arch}"
          ;;
      esac
      ;;
    linux)
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
      ;;
    *)
      die "unsupported os in resolve_target_triple: ${os}"
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

  if check_cmd sudo; then
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

downloader() {
  if [[ "${1:-}" == "--check" ]]; then
    if check_cmd curl || check_cmd wget; then
      return 0
    fi
    die "required command not found: curl or wget"
  fi

  local url="$1"
  local output="$2"

  if check_cmd curl; then
    local curl_args=(--fail --silent --show-error --location --output "${output}")
    if [[ -n "${GH_TOKEN:-}" ]]; then
      curl_args+=(--header "Authorization: Bearer ${GH_TOKEN}")
    fi
    curl "${curl_args[@]}" "${url}"
    return $?
  fi

  local wget_args=(--quiet "--output-document=${output}")
  if [[ -n "${GH_TOKEN:-}" ]]; then
    wget_args+=(--header="Authorization: Bearer ${GH_TOKEN}")
  fi
  wget "${wget_args[@]}" "${url}"
}

download_release_asset() {
  local url="$1"
  local output="$2"
  local mode="$3"

  if downloader "${url}" "${output}"; then
    return 0
  fi

  if [[ "${mode}" == "latest" ]]; then
    die "failed to download latest release asset: ${url} (latest release may not exist or asset may be missing; retry with --tag imagod-vX.Y.Z)"
  fi

  die "failed to download: ${url}"
}

verify_checksum() {
  local tmp_dir="$1"
  local checksum_name="$2"
  local asset_name="$3"

  if check_cmd sha256sum; then
    (
      cd "${tmp_dir}"
      sha256sum -c "${checksum_name}"
    ) || die "checksum verification failed for ${asset_name}"
    return 0
  fi

  if check_cmd shasum; then
    (
      cd "${tmp_dir}"
      shasum -a 256 -c "${checksum_name}"
    ) || die "checksum verification failed for ${asset_name}"
    return 0
  fi

  die "required checksum command not found: sha256sum or shasum"
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
  cat > "${unit_tmp}" <<EOF_SYSTEMD
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
EOF_SYSTEMD

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

  cat > "${init_tmp}" <<EOF_INITD
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
EOF_INITD

  if ! run_as_root install -m 0755 "${init_tmp}" /etc/init.d/imagod; then
    rm -f "${init_tmp}"
    warn "failed to install init.d script; keeping binary-only installation"
    return 1
  fi
  rm -f "${init_tmp}"

  if check_cmd update-rc.d; then
    run_as_root update-rc.d imagod defaults || warn "update-rc.d failed"
  elif check_cmd chkconfig; then
    run_as_root chkconfig --add imagod || warn "chkconfig --add failed"
  else
    warn "no init.d enable command found (update-rc.d/chkconfig); autostart not configured"
  fi

  if check_cmd service; then
    run_as_root service imagod start || warn "failed to start imagod via service command"
  else
    run_as_root /etc/init.d/imagod start || warn "failed to start /etc/init.d/imagod"
  fi

  log "init.d service installed (start attempted)"
  return 0
}

setup_launchd_system_daemon() {
  local binary_path="$1"
  local plist_tmp
  plist_tmp="$(mktemp)"

  cat > "${plist_tmp}" <<EOF_LAUNCHD
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>imagod</string>
  <key>ProgramArguments</key>
  <array>
    <string>${binary_path}</string>
    <string>--config</string>
    <string>${DEFAULT_CONFIG_PATH}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
EOF_LAUNCHD

  if ! run_as_root install -m 0644 "${plist_tmp}" "${LAUNCHD_PLIST_PATH}"; then
    rm -f "${plist_tmp}"
    warn "failed to install launchd plist; keeping binary-only installation"
    return 1
  fi
  rm -f "${plist_tmp}"

  run_as_root launchctl bootout system "${LAUNCHD_PLIST_PATH}" >/dev/null 2>&1 || true

  if ! run_as_root launchctl bootstrap system "${LAUNCHD_PLIST_PATH}"; then
    warn "launchctl bootstrap failed; keeping binary-only installation"
    return 1
  fi

  if ! run_as_root launchctl enable system/imagod; then
    warn "launchctl enable failed; keeping binary-only installation"
    return 1
  fi

  if ! run_as_root launchctl kickstart -k system/imagod; then
    warn "launchctl kickstart failed; keeping binary-only installation"
    return 1
  fi

  log "launchd system daemon installed and started: imagod"
  return 0
}

maybe_setup_service() {
  local binary_path="$1"
  local os="$2"

  if [[ "${NO_SERVICE}" -eq 1 ]]; then
    log "--no-service is set; skipped service setup"
    return 0
  fi

  if ! validate_service_binary_path "${binary_path}"; then
    return 0
  fi

  if [[ "${os}" == "darwin" ]]; then
    if check_cmd launchctl; then
      setup_launchd_system_daemon "${binary_path}" || true
      return 0
    fi

    warn "launchctl not found on macOS; binary installed only"
    return 0
  fi

  if check_cmd systemctl && [[ -d /run/systemd/system ]]; then
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
      --target)
        [[ $# -ge 2 ]] || die "--target requires a value"
        TARGET_OVERRIDE="$(validate_target_override "$2")"
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

  downloader --check
  need_cmd install
  need_cmd mktemp
  need_cmd uname

  local os
  local arch=""
  local libc=""
  local target
  local tag
  local release_mode

  os="$(detect_os)"

  if [[ -n "${TARGET_OVERRIDE}" ]]; then
    target="${TARGET_OVERRIDE}"
  else
    arch="$(detect_arch "${os}")"
    libc="$(detect_libc "${os}" "${arch}")"
    target="$(resolve_target_triple "${os}" "${arch}" "${libc}")"
  fi

  tag="$(resolve_release_tag)"
  if [[ -z "${tag}" && -z "${IMAGOD_RELEASE_BASE_URL:-}" ]]; then
    tag="$(resolve_latest_imagod_tag_from_git_refs)"
  fi

  if [[ -n "${tag}" ]]; then
    release_mode="tag"
  else
    release_mode="latest"
  fi

  local asset_name="imagod-${target}"
  local checksum_name="${asset_name}.sha256"
  local release_url_base
  release_url_base="$(resolve_release_url_base "${tag}")"
  local install_dir
  if [[ -n "${INSTALL_DIR}" ]]; then
    install_dir="${INSTALL_DIR}"
  else
    install_dir="$(default_install_dir)"
  fi
  validate_install_dir_path "${install_dir}"

  log "repository: ${REPO}"
  if [[ "${release_mode}" == "tag" ]]; then
    log "tag: ${tag}"
  else
    log "tag: latest"
  fi
  log "os: ${os}"
  log "target: ${target}"
  if [[ -n "${TARGET_OVERRIDE}" ]]; then
    log "target_resolution: override"
  else
    log "target_resolution: auto"
    if [[ "${os}" == "linux" ]]; then
      log "libc: ${libc}"
    fi
  fi
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

  download_release_asset "${release_url_base}/${asset_name}" "${tmp_dir}/${asset_name}" "${release_mode}"
  download_release_asset "${release_url_base}/${checksum_name}" "${tmp_dir}/${checksum_name}" "${release_mode}"
  verify_checksum "${tmp_dir}" "${checksum_name}" "${asset_name}"

  local installed_bin
  installed_bin="$(install_binary "${tmp_dir}/${asset_name}" "${install_dir}")"
  log "installed binary: ${installed_bin}"

  maybe_setup_service "${installed_bin}" "${os}"

  log "imagod installation completed"
  log "run '${installed_bin} --help' to verify installation"
}

main "$@"
