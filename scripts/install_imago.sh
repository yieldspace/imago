#!/bin/sh
set -eu

SCRIPT_NAME="install_imago.sh"
DEFAULT_RELEASES_API_URL="https://api.github.com/repos/yieldspace/imago/releases?per_page=100"
DEFAULT_RELEASE_DOWNLOAD_BASE="https://github.com/yieldspace/imago/releases/download"
GITHUB_USER_AGENT="imago-install-imago"

TAG_INPUT=""
TARGET_OVERRIDE=""
INSTALL_DIR=""
ALLOW_PRERELEASE=0
DRY_RUN=0
ASSUME_YES=0

RELEASES_API_URL="${IMAGO_RELEASES_API_URL:-${DEFAULT_RELEASES_API_URL}}"
RELEASE_BASE_URL_OVERRIDE="${IMAGO_RELEASE_BASE_URL:-}"
DRY_RUN_CHECK_ASSETS="${IMAGO_DRY_RUN_CHECK_ASSETS:-0}"
AUTH_TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
MAIN_TMP_DIR=""

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

cleanup_main_tmp_dir() {
  if [ -n "${MAIN_TMP_DIR}" ] && [ -d "${MAIN_TMP_DIR}" ]; then
    rm -rf "${MAIN_TMP_DIR}"
  fi
}

trap 'cleanup_main_tmp_dir' EXIT HUP INT TERM

usage() {
  cat <<'USAGE'
Usage: install_imago.sh [options]

Options:
  --tag <semver|imago-vX.Y.Z>       Install a specific release tag/version.
  --target <triple>                 Override auto-detected target triple.
  --install-dir <path>              Binary install directory.
  --prerelease                      Allow prerelease imago releases when --tag is omitted.
  -y, --yes                         Skip the confirmation prompt when a TTY is available.
  --dry-run                         Print resolved values without installing.
  -h, --help                        Show this help.

Environment:
  GH_TOKEN                          Optional GitHub token for API/download requests.
  GITHUB_TOKEN                      Fallback GitHub token for API/download requests.
  IMAGO_RELEASES_API_URL            Internal test override for the releases API URL.
  IMAGO_RELEASE_BASE_URL            Internal test override for the release asset base URL.
  IMAGO_DRY_RUN_CHECK_ASSETS=1      Internal test mode: verify asset URLs during --dry-run.

Notes:
  - Supported OS: Linux and macOS.
  - Release resolution: --tag > latest stable imago-v* release from GitHub Releases API.
  - Use --prerelease when the latest imago build is prerelease-only.
  - Interactive terminal runs ask for confirmation before installation; use -y to skip it.
USAGE
}

check_cmd() {
  command -v "$1" >/dev/null 2>&1
}

need_cmd() {
  if ! check_cmd "$1"; then
    die "required command not found: $1"
  fi
}

normalize_tag() {
  if printf '%s\n' "$1" | grep -Eq '^imago-v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$'; then
    printf '%s\n' "$1"
    return 0
  fi

  if printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$'; then
    printf 'imago-v%s\n' "$1"
    return 0
  fi

  die "invalid --tag value: $1"
}

validate_target_override() {
  if [ -z "$1" ]; then
    die "--target requires a non-empty value"
  fi

  case "$1" in
    *,*)
      die "invalid --target value: '$1' (comma-separated values are not supported)"
      ;;
  esac

  if ! printf '%s\n' "$1" | grep -Eq '^[A-Za-z0-9._-]+$'; then
    die "invalid --target value: '$1' (allowed characters: A-Z a-z 0-9 . _ -)"
  fi

  printf '%s\n' "$1"
}

resolve_home_dir() {
  if [ -n "${HOME:-}" ]; then
    printf '%s\n' "${HOME}"
    return 0
  fi

  if check_cmd getent; then
    home_user="$(id -un 2>/dev/null || true)"
    if [ -n "${home_user}" ]; then
      home_dir="$(getent passwd "${home_user}" | awk -F: 'NR == 1 { print $6 }')"
      if [ -n "${home_dir}" ]; then
        printf '%s\n' "${home_dir}"
        return 0
      fi
    fi
  fi

  die "HOME is not set and the home directory could not be resolved"
}

detect_os() {
  detected_uname_s="${IMAGO_TEST_UNAME_S:-$(uname -s)}"
  case "${detected_uname_s}" in
    Linux)
      printf 'linux\n'
      ;;
    Darwin)
      printf 'darwin\n'
      ;;
    *)
      die "unsupported OS: ${detected_uname_s} (supported: Linux, Darwin)"
      ;;
  esac
}

darwin_sysctl_has_one() {
  forced_value="$(eval "printf '%s' \"\${$2:-}\"")"
  if [ -n "${forced_value}" ]; then
    [ "${forced_value}" = "1" ]
    return $?
  fi

  if ! check_cmd sysctl; then
    return 1
  fi

  sysctl "$1" 2>/dev/null | grep -q ': 1'
}

detect_darwin_arch() {
  detected_uname_m="${IMAGO_TEST_UNAME_M:-$(uname -m)}"
  case "${detected_uname_m}" in
    i386)
      if darwin_sysctl_has_one "hw.optional.x86_64" "IMAGO_TEST_SYSCTL_HW_OPTIONAL_X86_64"; then
        detected_uname_m="x86_64"
      fi
      ;;
    x86_64|amd64)
      if darwin_sysctl_has_one "hw.optional.arm64" "IMAGO_TEST_SYSCTL_HW_OPTIONAL_ARM64"; then
        detected_uname_m="arm64"
      fi
      ;;
  esac

  case "${detected_uname_m}" in
    x86_64|amd64)
      printf 'x86_64\n'
      ;;
    arm64|aarch64)
      printf 'aarch64\n'
      ;;
    *)
      die "unsupported architecture on macOS: ${detected_uname_m}"
      ;;
  esac
}

detect_linux_arch() {
  detected_uname_m="${IMAGO_TEST_UNAME_M:-$(uname -m)}"
  case "${detected_uname_m}" in
    x86_64|amd64)
      printf 'x86_64\n'
      ;;
    aarch64|arm64)
      printf 'aarch64\n'
      ;;
    armv7l|armv7hf|armv7)
      printf 'armv7\n'
      ;;
    riscv64|riscv64gc)
      printf 'riscv64gc\n'
      ;;
    *)
      die "unsupported architecture on Linux: ${detected_uname_m}"
      ;;
  esac
}

detect_arch() {
  case "$1" in
    linux)
      detect_linux_arch
      ;;
    darwin)
      detect_darwin_arch
      ;;
    *)
      die "unsupported os in detect_arch: $1"
      ;;
  esac
}

has_matching_path() {
  for matching_path in "$@"; do
    if [ -e "${matching_path}" ]; then
      return 0
    fi
  done
  return 1
}

detect_linux_libc() {
  if [ -n "${IMAGO_TEST_LIBC:-}" ]; then
    case "${IMAGO_TEST_LIBC}" in
      gnu|musl)
        printf '%s\n' "${IMAGO_TEST_LIBC}"
        return 0
        ;;
      *)
        die "IMAGO_TEST_LIBC must be 'gnu' or 'musl'"
        ;;
    esac
  fi

  if check_cmd ldd; then
    ldd_out="$(ldd --version 2>&1 || true)"
    if printf '%s\n' "${ldd_out}" | grep -qi 'musl'; then
      printf 'musl\n'
      return 0
    fi
    if printf '%s\n' "${ldd_out}" | grep -qiE '(glibc|gnu libc)'; then
      printf 'gnu\n'
      return 0
    fi
  fi

  if check_cmd getconf && getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
    printf 'gnu\n'
    return 0
  fi

  if has_matching_path /lib/ld-musl-*.so.* /lib64/ld-musl-*.so.* /usr/lib/ld-musl-*.so.*; then
    warn "libc auto-detection inferred 'musl' from dynamic loader files; rerun with --target <triple> to override"
    printf 'musl\n'
    return 0
  fi

  if has_matching_path /lib/ld-linux*.so* /lib64/ld-linux*.so* /usr/lib/ld-linux*.so* /usr/lib64/ld-linux*.so*; then
    warn "libc auto-detection inferred 'gnu' from dynamic loader files; rerun with --target <triple> to override"
    printf 'gnu\n'
    return 0
  fi

  case "$1" in
    aarch64|armv7|riscv64gc)
      warn "libc auto-detection is inconclusive; falling back to 'musl' for arch=$1. rerun with --target <triple> if needed"
      printf 'musl\n'
      ;;
    x86_64)
      warn "libc auto-detection is inconclusive; falling back to 'gnu' for arch=$1. rerun with --target <triple> if needed"
      printf 'gnu\n'
      ;;
    *)
      die "libc auto-detection failed for arch=$1; rerun with --target <triple>"
      ;;
  esac
}

detect_libc() {
  if [ "$1" != "linux" ]; then
    printf 'none\n'
    return 0
  fi

  detect_linux_libc "$2"
}

resolve_target_triple() {
  case "$1:$2:$3" in
    darwin:x86_64:none)
      printf 'x86_64-apple-darwin\n'
      ;;
    darwin:aarch64:none)
      printf 'aarch64-apple-darwin\n'
      ;;
    linux:x86_64:gnu)
      printf 'x86_64-unknown-linux-gnu\n'
      ;;
    linux:x86_64:musl)
      printf 'x86_64-unknown-linux-musl\n'
      ;;
    linux:aarch64:gnu)
      printf 'aarch64-unknown-linux-gnu\n'
      ;;
    linux:aarch64:musl)
      printf 'aarch64-unknown-linux-musl\n'
      ;;
    linux:armv7:gnu)
      printf 'armv7-unknown-linux-gnueabihf\n'
      ;;
    linux:armv7:musl)
      printf 'armv7-unknown-linux-musleabihf\n'
      ;;
    linux:riscv64gc:gnu)
      printf 'riscv64gc-unknown-linux-gnu\n'
      ;;
    linux:riscv64gc:musl)
      printf 'riscv64gc-unknown-linux-musl\n'
      ;;
    *)
      die "unsupported target combination: os=$1 arch=$2 libc=$3"
      ;;
  esac
}

default_install_dir() {
  if [ "$(id -u)" -eq 0 ]; then
    printf '/usr/local/bin\n'
    return 0
  fi

  home_dir="$(resolve_home_dir)"
  printf '%s/.local/bin\n' "${home_dir}"
}

run_as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
    return 0
  fi

  if check_cmd sudo; then
    sudo "$@"
    return 0
  fi

  return 1
}

validate_install_dir_path() {
  if [ -z "$1" ]; then
    die "--install-dir requires a non-empty value"
  fi

  if ! printf '%s\n' "$1" | grep -Eq '^[A-Za-z0-9._/+:-]+$'; then
    die "invalid --install-dir: '$1' (allowed characters: A-Z a-z 0-9 . _ / + : -)"
  fi
}

downloader() {
  if [ "${1:-}" = "--check" ]; then
    if check_cmd curl || check_cmd wget; then
      return 0
    fi
    die "required command not found: curl or wget"
  fi

  if check_cmd curl; then
    if [ -n "${AUTH_TOKEN}" ]; then
      curl --fail --silent --show-error --location --user-agent "${GITHUB_USER_AGENT}" --header "Authorization: Bearer ${AUTH_TOKEN}" --output "$2" "$1"
    else
      curl --fail --silent --show-error --location --user-agent "${GITHUB_USER_AGENT}" --output "$2" "$1"
    fi
    return $?
  fi

  if [ -n "${AUTH_TOKEN}" ]; then
    wget --quiet --user-agent="${GITHUB_USER_AGENT}" --header="Authorization: Bearer ${AUTH_TOKEN}" --output-document="$2" "$1"
  else
    wget --quiet --user-agent="${GITHUB_USER_AGENT}" --output-document="$2" "$1"
  fi
}

download_github_api() {
  if check_cmd curl; then
    if [ -n "${AUTH_TOKEN}" ]; then
      curl --fail --silent --show-error --location --user-agent "${GITHUB_USER_AGENT}" --header "Accept: application/vnd.github+json" --header "X-GitHub-Api-Version: 2022-11-28" --header "Authorization: Bearer ${AUTH_TOKEN}" --output "$2" "$1"
    else
      curl --fail --silent --show-error --location --user-agent "${GITHUB_USER_AGENT}" --header "Accept: application/vnd.github+json" --header "X-GitHub-Api-Version: 2022-11-28" --output "$2" "$1"
    fi
    return $?
  fi

  if [ -n "${AUTH_TOKEN}" ]; then
    wget --quiet --user-agent="${GITHUB_USER_AGENT}" --header="Accept: application/vnd.github+json" --header="X-GitHub-Api-Version: 2022-11-28" --header="Authorization: Bearer ${AUTH_TOKEN}" --output-document="$2" "$1"
  else
    wget --quiet --user-agent="${GITHUB_USER_AGENT}" --header="Accept: application/vnd.github+json" --header="X-GitHub-Api-Version: 2022-11-28" --output-document="$2" "$1"
  fi
}

parse_release_tag_from_index() {
  tr '\r\n' '  ' < "$1" |
    grep -Eo '"tag_name"[[:space:]]*:[[:space:]]*"[^"]+"|"draft"[[:space:]]*:[[:space:]]*(true|false)|"prerelease"[[:space:]]*:[[:space:]]*(true|false)' |
    awk -v allow_prerelease="$2" '
    function reset_release() {
      tag = ""
      draft = ""
      prerelease = ""
    }
    function emit_if_match() {
      if (tag == "") {
        return
      }
      if (tag !~ /^imago-v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$/) {
        reset_release()
        return
      }
      if (draft == "false" && (allow_prerelease == "1" || prerelease == "false")) {
        print tag
        found = 1
        exit
      }
      reset_release()
    }
    BEGIN {
      reset_release()
    }
    /^"tag_name"[[:space:]]*:/ {
      if (tag != "" && draft != "" && prerelease != "") {
        emit_if_match()
      }
      line = $0
      sub(/.*"tag_name"[[:space:]]*:[[:space:]]*"/, "", line)
      sub(/"$/, "", line)
      tag = line
    }
    /^"draft"[[:space:]]*:/ {
      line = $0
      sub(/.*"draft"[[:space:]]*:[[:space:]]*/, "", line)
      draft = line
    }
    /^"prerelease"[[:space:]]*:/ {
      line = $0
      sub(/.*"prerelease"[[:space:]]*:[[:space:]]*/, "", line)
      prerelease = line
      if (tag != "" && draft != "") {
        emit_if_match()
      }
    }
    END {
      if (!found && tag != "" && draft != "" && prerelease != "") {
        emit_if_match()
      }
    }
  '
}

release_api_supports_paging() {
  case "$1" in
    http://*|https://*)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

release_api_page_url() {
  base_url="$1"
  page="$2"

  if [ "${page}" = "1" ] || ! release_api_supports_paging "${base_url}"; then
    printf '%s\n' "${base_url}"
    return 0
  fi

  case "${base_url}" in
    *\?*)
      printf '%s&page=%s\n' "${base_url}" "${page}"
      ;;
    *)
      printf '%s?page=%s\n' "${base_url}" "${page}"
      ;;
  esac
}

resolve_latest_release_tag() {
  page=1
  supports_paging=0
  if release_api_supports_paging "${RELEASES_API_URL}"; then
    supports_paging=1
  fi

  while :; do
    release_index_tmp="$(mktemp)"
    api_url="$(release_api_page_url "${RELEASES_API_URL}" "${page}")"

    if ! download_github_api "${api_url}" "${release_index_tmp}"; then
      rm -f "${release_index_tmp}"
      die "failed to query GitHub Releases API: ${api_url} (set GH_TOKEN/GITHUB_TOKEN or pass --tag imago-vX.Y.Z)"
    fi

    if grep -q '"tag_name"' "${release_index_tmp}"; then
      has_release_items=1
    else
      has_release_items=0
      if [ "${page}" = "1" ] && ! grep -Eq '^[[:space:]]*\[' "${release_index_tmp}"; then
        rm -f "${release_index_tmp}"
        die "failed to parse GitHub Releases API response from ${api_url}; pass --tag imago-vX.Y.Z explicitly"
      fi
    fi

    selected_tag="$(parse_release_tag_from_index "${release_index_tmp}" "${ALLOW_PRERELEASE}")"
    rm -f "${release_index_tmp}"

    if [ -n "${selected_tag}" ]; then
      printf '%s\n' "${selected_tag}"
      return 0
    fi

    if [ "${supports_paging}" != "1" ] || [ "${has_release_items}" != "1" ]; then
      break
    fi

    page=$((page + 1))
  done

  if [ "${ALLOW_PRERELEASE}" = "1" ]; then
    die "no imago release found via GitHub Releases API; pass --tag imago-vX.Y.Z explicitly"
  fi

  die "no stable imago release found via GitHub Releases API; rerun with --prerelease or pass --tag imago-vX.Y.Z"
}

resolve_release_url_base() {
  if [ -n "${RELEASE_BASE_URL_OVERRIDE}" ]; then
    printf '%s\n' "${RELEASE_BASE_URL_OVERRIDE}"
    return 0
  fi

  printf '%s/%s\n' "${DEFAULT_RELEASE_DOWNLOAD_BASE}" "$1"
}

download_release_asset() {
  release_asset_url="$1"
  release_asset_output="$2"
  release_selection_mode="$3"
  release_resolved_tag="$4"
  release_asset_name="$5"

  if downloader "${release_asset_url}" "${release_asset_output}"; then
    return 0
  fi

  case "${release_selection_mode}" in
    stable)
      die "resolved stable release ${release_resolved_tag} does not provide ${release_asset_name} yet; retry later, use --prerelease, or pass --tag imago-vX.Y.Z"
      ;;
    prerelease)
      die "resolved prerelease ${release_resolved_tag} does not provide ${release_asset_name} yet; retry later or pass --tag imago-vX.Y.Z"
      ;;
    *)
      die "failed to download ${release_asset_name} from ${release_asset_url}"
      ;;
  esac
}

verify_checksum() {
  verify_checksum_dir="$1"
  verify_checksum_name="$2"
  verify_asset_name="$3"

  if check_cmd sha256sum; then
    (
      cd "${verify_checksum_dir}"
      sha256sum -c "${verify_checksum_name}"
    ) || die "checksum verification failed for ${verify_asset_name}"
    return 0
  fi

  if check_cmd shasum; then
    (
      cd "${verify_checksum_dir}"
      shasum -a 256 -c "${verify_checksum_name}"
    ) || die "checksum verification failed for ${verify_asset_name}"
    return 0
  fi

  die "required checksum command not found: sha256sum or shasum"
}

check_release_assets_for_dry_run() {
  if [ "${DRY_RUN_CHECK_ASSETS}" != "1" ]; then
    return 0
  fi

  (
    probe_tmp_dir="$(mktemp -d)"
    trap 'rm -rf "${probe_tmp_dir}"' EXIT HUP INT TERM
    download_release_asset "$1" "${probe_tmp_dir}/$3" "$5" "$6" "$3"
    download_release_asset "$2" "${probe_tmp_dir}/$4" "$5" "$6" "$4"
  )

  log "dry_run_asset_check: ok"
}

install_binary() {
  install_source_bin="$1"
  install_destination_dir="$2"
  install_destination_bin="${install_destination_dir}/imago"

  if mkdir -p -- "${install_destination_dir}" 2>/dev/null && install -m 0755 -- "${install_source_bin}" "${install_destination_bin}" 2>/dev/null; then
    printf '%s\n' "${install_destination_bin}"
    return 0
  fi

  run_as_root install -d -- "${install_destination_dir}" || die "failed to create install dir: ${install_destination_dir}"
  run_as_root install -m 0755 -- "${install_source_bin}" "${install_destination_bin}" || die "failed to install imago to ${install_destination_bin}"
  printf '%s\n' "${install_destination_bin}"
}

path_contains() {
  path_candidate="$1"
  old_ifs="${IFS}"
  IFS=':'
  for path_entry in ${PATH:-}; do
    if [ "${path_entry}" = "${path_candidate}" ]; then
      IFS="${old_ifs}"
      return 0
    fi
  done
  IFS="${old_ifs}"
  return 1
}

tty_prompt_available() {
  if [ "${ASSUME_YES}" = "1" ] || [ "${DRY_RUN}" = "1" ]; then
    return 1
  fi

  if [ ! -t 1 ] && [ ! -t 2 ]; then
    return 1
  fi

  if ! ( : >/dev/tty ) 2>/dev/null || ! ( : </dev/tty ) 2>/dev/null; then
    return 1
  fi

  return 0
}

confirm_installation_or_exit() {
  confirm_tag="$1"
  confirm_release_resolution="$2"
  confirm_os="$3"
  confirm_target="$4"
  confirm_target_resolution="$5"
  confirm_libc="$6"
  confirm_install_dir="$7"

  if ! tty_prompt_available; then
    return 0
  fi

  {
    printf '%s\n' "Install imago with the following settings?"
    printf '  tag: %s\n' "${confirm_tag}"
    printf '  release_resolution: %s\n' "${confirm_release_resolution}"
    printf '  os: %s\n' "${confirm_os}"
    printf '  target: %s\n' "${confirm_target}"
    printf '  target_resolution: %s\n' "${confirm_target_resolution}"
    if [ "${confirm_target_resolution}" = "auto" ] && [ "${confirm_os}" = "linux" ]; then
      printf '  libc: %s\n' "${confirm_libc}"
    fi
    printf '  install_dir: %s\n' "${confirm_install_dir}"
    printf 'Proceed with installation? [y/N] '
  } >/dev/tty

  if ! IFS= read -r confirm_reply </dev/tty; then
    confirm_reply=""
  fi

  case "${confirm_reply}" in
    y|Y|yes|YES|Yes)
      return 0
      ;;
    *)
      log "installation cancelled by user"
      exit 0
      ;;
  esac
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --tag)
        [ "$#" -ge 2 ] || die "--tag requires a value"
        TAG_INPUT="$2"
        shift 2
        ;;
      --tag=*)
        TAG_INPUT="${1#--tag=}"
        shift
        ;;
      --target)
        [ "$#" -ge 2 ] || die "--target requires a value"
        TARGET_OVERRIDE="$(validate_target_override "$2")"
        shift 2
        ;;
      --target=*)
        TARGET_OVERRIDE="$(validate_target_override "${1#--target=}")"
        shift
        ;;
      --install-dir)
        [ "$#" -ge 2 ] || die "--install-dir requires a value"
        INSTALL_DIR="$2"
        shift 2
        ;;
      --install-dir=*)
        INSTALL_DIR="${1#--install-dir=}"
        shift
        ;;
      --prerelease)
        ALLOW_PRERELEASE=1
        shift
        ;;
      -y|--yes)
        ASSUME_YES=1
        shift
        ;;
      --dry-run)
        DRY_RUN=1
        shift
        ;;
      -h|--help)
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
  need_cmd awk
  need_cmd grep
  need_cmd id
  need_cmd install
  need_cmd mkdir
  need_cmd mktemp
  need_cmd rm
  need_cmd tr
  need_cmd uname

  os="$(detect_os)"
  arch=""
  libc=""
  target=""

  if [ -n "${TARGET_OVERRIDE}" ]; then
    target="${TARGET_OVERRIDE}"
    target_resolution="override"
  else
    arch="$(detect_arch "${os}")"
    libc="$(detect_libc "${os}" "${arch}")"
    target="$(resolve_target_triple "${os}" "${arch}" "${libc}")"
    target_resolution="auto"
  fi

  if [ -n "${TAG_INPUT}" ]; then
    resolved_tag="$(normalize_tag "${TAG_INPUT}")"
    selection_mode="tag"
  else
    resolved_tag="$(resolve_latest_release_tag)"
    if [ "${ALLOW_PRERELEASE}" = "1" ]; then
      selection_mode="prerelease"
    else
      selection_mode="stable"
    fi
  fi

  asset_name="imago-${target}"
  checksum_name="${asset_name}.sha256"
  release_url_base="$(resolve_release_url_base "${resolved_tag}")"
  binary_url="${release_url_base}/${asset_name}"
  checksum_url="${release_url_base}/${checksum_name}"

  if [ -n "${INSTALL_DIR}" ]; then
    install_dir="${INSTALL_DIR}"
  else
    install_dir="$(default_install_dir)"
  fi
  validate_install_dir_path "${install_dir}"

  log "tag: ${resolved_tag}"
  log "release_resolution: ${selection_mode}"
  log "os: ${os}"
  log "target: ${target}"
  log "target_resolution: ${target_resolution}"
  if [ "${target_resolution}" = "auto" ] && [ "${os}" = "linux" ]; then
    log "libc: ${libc}"
  fi
  log "install_dir: ${install_dir}"

  if [ "${DRY_RUN}" = "1" ]; then
    check_release_assets_for_dry_run "${binary_url}" "${checksum_url}" "${asset_name}" "${checksum_name}" "${selection_mode}" "${resolved_tag}"
    log "dry-run enabled; no changes applied"
    log "binary URL: ${binary_url}"
    log "checksum URL: ${checksum_url}"
    exit 0
  fi

  confirm_installation_or_exit "${resolved_tag}" "${selection_mode}" "${os}" "${target}" "${target_resolution}" "${libc}" "${install_dir}"

  MAIN_TMP_DIR="$(mktemp -d)"
  download_release_asset "${binary_url}" "${MAIN_TMP_DIR}/${asset_name}" "${selection_mode}" "${resolved_tag}" "${asset_name}"
  download_release_asset "${checksum_url}" "${MAIN_TMP_DIR}/${checksum_name}" "${selection_mode}" "${resolved_tag}" "${checksum_name}"
  verify_checksum "${MAIN_TMP_DIR}" "${checksum_name}" "${asset_name}"

  installed_bin="$(install_binary "${MAIN_TMP_DIR}/${asset_name}" "${install_dir}")"
  log "installed binary: ${installed_bin}"

  if ! path_contains "${install_dir}"; then
    log "PATH hint: add ${install_dir} to PATH"
  fi

  log "imago installation completed"
  log "run '${installed_bin} --help' to verify installation"
}

main "$@"
