#!/bin/sh
set -eu

SCRIPT_NAME="install_imagod.sh"
DEFAULT_CONFIG_PATH="/etc/imago/imagod.toml"
LAUNCHD_PLIST_PATH="/Library/LaunchDaemons/imagod.plist"
DEFAULT_RELEASES_API_URL="https://api.github.com/repos/yieldspace/imago/releases?per_page=100"
DEFAULT_RELEASE_TAG_API_BASE="https://api.github.com/repos/yieldspace/imago/releases/tags"
DEFAULT_RELEASE_DOWNLOAD_BASE="https://github.com/yieldspace/imago/releases/download"
GITHUB_USER_AGENT="imago-install-imagod"

TAG_INPUT=""
TARGET_OVERRIDE=""
FEATURES_OVERRIDE=""
FEATURES_EXPLICIT=0
INSTALL_DIR=""
ALLOW_PRERELEASE=0
WITH_SERVICE=0
DRY_RUN=0
ASSUME_YES=0

RELEASES_API_URL="${IMAGOD_RELEASES_API_URL:-${DEFAULT_RELEASES_API_URL}}"
RELEASE_TAG_API_BASE="${IMAGOD_RELEASE_TAG_API_BASE:-${DEFAULT_RELEASE_TAG_API_BASE}}"
RELEASE_BASE_URL_OVERRIDE="${IMAGOD_RELEASE_BASE_URL:-}"
RELEASE_METADATA_URL_OVERRIDE="${IMAGOD_RELEASE_METADATA_URL:-}"
DRY_RUN_CHECK_ASSETS="${IMAGOD_DRY_RUN_CHECK_ASSETS:-0}"
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
Usage: install_imagod.sh [options]

Options:
  --tag <semver|imagod-vX.Y.Z>      Install a specific release tag/version.
  --target <triple>                 Override auto-detected target triple.
  --features <csv>                  Select a feature variant such as wasi-nn-cvitek.
  --install-dir <path>              Binary install directory.
  --prerelease                      Allow prerelease imagod releases when --tag is omitted.
  --with-service                    Install and start a service after installing the binary.
  -y, --yes                         Skip TTY prompts and install the default variant.
  --dry-run                         Print resolved values without installing.
  -h, --help                        Show this help.

Environment:
  GH_TOKEN                          Optional GitHub token for API/download requests.
  GITHUB_TOKEN                      Fallback GitHub token for API/download requests.
  IMAGOD_RELEASES_API_URL           Internal test override for the releases API URL.
  IMAGOD_RELEASE_TAG_API_BASE       Internal test override for the release-by-tag API base URL.
  IMAGOD_RELEASE_BASE_URL           Internal test override for the release asset base URL.
  IMAGOD_RELEASE_METADATA_URL       Internal test override for a specific release metadata URL.
  IMAGOD_DRY_RUN_CHECK_ASSETS=1     Internal test mode: verify asset URLs during --dry-run.

Notes:
  - Supported OS: Linux and macOS.
  - Release resolution: --tag > latest stable imagod-v* release from GitHub Releases API.
  - imagod feature variants are published as imagod-<target>+<feature1>+<feature2>.
  - Use --prerelease when the latest imagod build is still prerelease-only.
  - Service setup is disabled by default. Use --with-service to opt in.
  - Interactive terminal runs show release variants with the auto-detected imagod-<target> entry preselected; use -y to skip the prompt.
  - SSH targets call `ssh <host> imagod proxy-stdio` on the remote host.
  - The default SSH control socket is /run/imago/imagod.sock and can be overridden in imagod.toml with control_socket_path.
  - --with-service installs a service that reads /etc/imago/imagod.toml; prepare that config separately before first start.
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
  if printf '%s\n' "$1" | grep -Eq '^imagod-v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$'; then
    printf '%s\n' "$1"
    return 0
  fi

  if printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$'; then
    printf 'imagod-v%s\n' "$1"
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

normalize_features_csv() {
  if [ -z "$1" ]; then
    printf '\n'
    return 0
  fi

  normalized_features="$(
    printf '%s' "$1" |
      tr ',' '\n' |
      awk '
        {
          gsub(/^[[:space:]]+/, "", $0)
          gsub(/[[:space:]]+$/, "", $0)
          if ($0 == "") {
            next
          }
          if ($0 !~ /^[A-Za-z0-9._-]+$/) {
            printf "invalid feature name: %s\n", $0 > "/dev/stderr"
            exit 2
          }
          print
        }
      ' |
      sort -u |
      awk 'BEGIN { sep = "" } { printf "%s%s", sep, $0; sep = "," } END { printf "\n" }'
  )" || die "invalid --features value: '$1' (use comma-separated feature names with A-Z a-z 0-9 . _ -)"

  printf '%s\n' "${normalized_features}"
}

feature_csv_to_plus_suffix() {
  if [ -z "$1" ]; then
    printf '\n'
    return 0
  fi

  printf '+%s\n' "$(printf '%s' "$1" | tr ',' '+')"
}

feature_csv_display() {
  if [ -n "$1" ]; then
    printf '%s\n' "$1"
    return 0
  fi

  printf '<none>\n'
}

feature_csv_contains() {
  feature_csv="$1"
  wanted_feature="$2"

  if [ -z "${feature_csv}" ]; then
    return 1
  fi

  printf '%s' "${feature_csv}" | tr ',' '\n' | grep -Fx "${wanted_feature}" >/dev/null 2>&1
}

imagod_asset_name_for_variant() {
  asset_target="$1"
  asset_features_csv="$2"
  asset_suffix="$(feature_csv_to_plus_suffix "${asset_features_csv}")"
  printf 'imagod-%s%s\n' "${asset_target}" "${asset_suffix}"
}

parse_imagod_asset_name() {
  case "$1" in
    imagod-*)
      ;;
    *)
      return 1
      ;;
  esac

  parsed_rest="${1#imagod-}"
  case "${parsed_rest}" in
    *+*)
      parsed_target="${parsed_rest%%+*}"
      parsed_feature_plus="${parsed_rest#*+}"
      ;;
    *)
      parsed_target="${parsed_rest}"
      parsed_feature_plus=""
      ;;
  esac

  if ! printf '%s\n' "${parsed_target}" | grep -Eq '^[A-Za-z0-9._-]+$'; then
    return 1
  fi
  if [ -n "${parsed_feature_plus}" ] && ! printf '%s\n' "${parsed_feature_plus}" | grep -Eq '^[A-Za-z0-9._-]+(\+[A-Za-z0-9._-]+)*$'; then
    return 1
  fi

  printf '%s\t%s\n' "${parsed_target}" "$(printf '%s' "${parsed_feature_plus}" | tr '+' ',')"
}

target_os_family() {
  case "$1" in
    *-apple-darwin)
      printf 'darwin\n'
      ;;
    *-unknown-linux-*)
      printf 'linux\n'
      ;;
    *)
      printf 'unknown\n'
      ;;
  esac
}

target_arch_family() {
  case "$1" in
    x86_64-*)
      printf 'x86_64\n'
      ;;
    aarch64-*)
      printf 'aarch64\n'
      ;;
    armv7-*)
      printf 'armv7\n'
      ;;
    riscv64gc-*)
      printf 'riscv64gc\n'
      ;;
    *)
      printf 'unknown\n'
      ;;
  esac
}

target_libc_family() {
  case "$1" in
    *-linux-musl*)
      printf 'musl\n'
      ;;
    *-linux-gnu*)
      printf 'gnu\n'
      ;;
    *-apple-darwin)
      printf 'darwin\n'
      ;;
    *)
      printf 'unknown\n'
      ;;
  esac
}

variant_candidate_priority() {
  candidate_target="$1"
  candidate_features="$2"
  desired_target="$3"
  desired_features="$4"

  if [ "${candidate_target}" = "${desired_target}" ] && [ "${candidate_features}" = "${desired_features}" ]; then
    printf '0\n'
    return 0
  fi
  if [ "${candidate_target}" = "${desired_target}" ] && [ -z "${candidate_features}" ]; then
    printf '1\n'
    return 0
  fi
  if [ "${candidate_target}" = "${desired_target}" ]; then
    printf '2\n'
    return 0
  fi

  candidate_os="$(target_os_family "${candidate_target}")"
  desired_os="$(target_os_family "${desired_target}")"
  candidate_arch="$(target_arch_family "${candidate_target}")"
  desired_arch="$(target_arch_family "${desired_target}")"
  candidate_libc="$(target_libc_family "${candidate_target}")"
  desired_libc="$(target_libc_family "${desired_target}")"

  if [ "${candidate_os}" = "${desired_os}" ] && [ "${candidate_arch}" = "${desired_arch}" ] && [ "${candidate_libc}" = "${desired_libc}" ]; then
    printf '3\n'
    return 0
  fi
  if [ "${candidate_os}" = "${desired_os}" ] && [ "${candidate_arch}" = "${desired_arch}" ]; then
    printf '4\n'
    return 0
  fi
  if [ "${candidate_os}" = "${desired_os}" ]; then
    printf '5\n'
    return 0
  fi

  printf '6\n'
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
  detected_uname_s="${IMAGOD_TEST_UNAME_S:-$(uname -s)}"
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
  detected_uname_m="${IMAGOD_TEST_UNAME_M:-$(uname -m)}"
  case "${detected_uname_m}" in
    i386)
      if darwin_sysctl_has_one "hw.optional.x86_64" "IMAGOD_TEST_SYSCTL_HW_OPTIONAL_X86_64"; then
        detected_uname_m="x86_64"
      fi
      ;;
    x86_64|amd64)
      if darwin_sysctl_has_one "hw.optional.arm64" "IMAGOD_TEST_SYSCTL_HW_OPTIONAL_ARM64"; then
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
  detected_uname_m="${IMAGOD_TEST_UNAME_M:-$(uname -m)}"
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
  if [ -n "${IMAGOD_TEST_LIBC:-}" ]; then
    case "${IMAGOD_TEST_LIBC}" in
      gnu|musl)
        printf '%s\n' "${IMAGOD_TEST_LIBC}"
        return 0
        ;;
      *)
        die "IMAGOD_TEST_LIBC must be 'gnu' or 'musl'"
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

validate_service_binary_path_or_die() {
  case "$1" in
    /*)
      ;;
    *)
      die "--with-service requires an absolute install path; use an absolute --install-dir"
      ;;
  esac

  if ! printf '%s\n' "$1" | grep -Eq '^/[A-Za-z0-9._/+:-]+$'; then
    die "service binary path contains unsafe characters: $1"
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
      if (tag !~ /^imagod-v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$/) {
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
      die "failed to query GitHub Releases API: ${api_url} (set GH_TOKEN/GITHUB_TOKEN or pass --tag imagod-vX.Y.Z)"
    fi

    if grep -q '"tag_name"' "${release_index_tmp}"; then
      has_release_items=1
    else
      has_release_items=0
      if [ "${page}" = "1" ] && ! grep -Eq '^[[:space:]]*\[' "${release_index_tmp}"; then
        rm -f "${release_index_tmp}"
        die "failed to parse GitHub Releases API response from ${api_url}; pass --tag imagod-vX.Y.Z explicitly"
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
    die "no imagod release found via GitHub Releases API; pass --tag imagod-vX.Y.Z explicitly"
  fi

  die "no stable imagod release found via GitHub Releases API; rerun with --prerelease or pass --tag imagod-vX.Y.Z"
}

resolve_release_url_base() {
  if [ -n "${RELEASE_BASE_URL_OVERRIDE}" ]; then
    printf '%s\n' "${RELEASE_BASE_URL_OVERRIDE}"
    return 0
  fi

  printf '%s/%s\n' "${DEFAULT_RELEASE_DOWNLOAD_BASE}" "$1"
}

resolve_release_metadata_url() {
  release_metadata_tag="$1"

  if [ -n "${RELEASE_METADATA_URL_OVERRIDE}" ]; then
    printf '%s\n' "${RELEASE_METADATA_URL_OVERRIDE}"
    return 0
  fi

  printf '%s/%s\n' "${RELEASE_TAG_API_BASE}" "${release_metadata_tag}"
}

download_release_metadata() {
  release_metadata_tag="$1"
  release_metadata_output="$2"
  release_metadata_url="$(resolve_release_metadata_url "${release_metadata_tag}")"

  if download_github_api "${release_metadata_url}" "${release_metadata_output}"; then
    return 0
  fi

  die "failed to query GitHub release metadata: ${release_metadata_url} (set GH_TOKEN/GITHUB_TOKEN or pass IMAGOD_RELEASE_METADATA_URL for tests)"
}

parse_release_asset_names_from_metadata() {
  tr '\r\n' '  ' < "$1" |
    awk '
      {
        line = $0

        while (match(line, /"assets"[[:space:]]*:[[:space:]]*\[|\{|\}|\[|\]|"name"[[:space:]]*:[[:space:]]*"[^"]+"/)) {
          token = substr(line, RSTART, RLENGTH)
          line = substr(line, RSTART + RLENGTH)

          if (!in_assets) {
            if (token ~ /^"assets"[[:space:]]*:/) {
              in_assets = 1
              object_depth = 0
              array_depth = 1
            }
            continue
          }

          if (token == "{") {
            object_depth += 1
            continue
          }

          if (token == "}") {
            if (object_depth > 0) {
              object_depth -= 1
            }
            continue
          }

          if (token == "[") {
            array_depth += 1
            continue
          }

          if (token == "]") {
            if (array_depth > 0) {
              array_depth -= 1
            }
            if (array_depth == 0) {
              in_assets = 0
            }
            continue
          }

          if (object_depth == 1 && array_depth == 1 && token ~ /^"name"[[:space:]]*:/) {
            sub(/^"name"[[:space:]]*:[[:space:]]*"/, "", token)
            sub(/"$/, "", token)
            print token
          }
        }
      }
    '
}

catalog_has_asset_name() {
  awk -F '\t' -v asset_name="$2" '
    $2 == asset_name {
      found = 1
      exit
    }
    END {
      exit(found ? 0 : 1)
    }
  ' "$1"
}

render_variant_candidates() {
  awk -F '\t' '
    {
      features = ($4 == "" ? "<none>" : $4)
      printf "  - %s (target=%s, features=%s)\n", $2, $3, features
    }
  ' "$1"
}

build_release_variant_catalog() {
  release_metadata_file="$1"
  desired_target="$2"
  desired_features="$3"
  catalog_output="$4"
  release_assets_file="${MAIN_TMP_DIR}/release-assets.txt"
  catalog_unsorted_file="${MAIN_TMP_DIR}/release-variants.unsorted.txt"
  tab_char="$(printf '\t')"

  parse_release_asset_names_from_metadata "${release_metadata_file}" | sort -u > "${release_assets_file}"
  : > "${catalog_unsorted_file}"

  while IFS= read -r release_asset_name; do
    case "${release_asset_name}" in
      imagod-*.sha256)
        continue
        ;;
      imagod-*)
        checksum_asset_name="${release_asset_name}.sha256"
        if ! grep -Fx "${checksum_asset_name}" "${release_assets_file}" >/dev/null 2>&1; then
          continue
        fi

        parsed_variant="$(parse_imagod_asset_name "${release_asset_name}")" || continue
        variant_target="$(printf '%s' "${parsed_variant}" | awk -F '\t' 'NR == 1 { print $1 }')"
        variant_features="$(printf '%s' "${parsed_variant}" | awk -F '\t' 'NR == 1 { print $2 }')"
        variant_priority="$(variant_candidate_priority "${variant_target}" "${variant_features}" "${desired_target}" "${desired_features}")"
        printf '%s\t%s\t%s\t%s\n' "${variant_priority}" "${release_asset_name}" "${variant_target}" "${variant_features}" >> "${catalog_unsorted_file}"
        ;;
    esac
  done < "${release_assets_file}"

  sort -t "${tab_char}" -k1,1n -k2,2 "${catalog_unsorted_file}" > "${catalog_output}"

  if [ ! -s "${catalog_output}" ]; then
    die "release metadata does not contain any imagod assets with matching .sha256 entries"
  fi
}

die_with_variant_candidates() {
  requested_asset_name="$1"
  requested_tag="$2"
  catalog_file="$3"

  {
    printf '[%s] error: requested imagod variant %s is not available in %s\n' "${SCRIPT_NAME}" "${requested_asset_name}" "${requested_tag}"
    printf '[%s] error: available variants:\n' "${SCRIPT_NAME}"
    render_variant_candidates "${catalog_file}"
  } >&2
  exit 1
}

select_release_asset_or_exit() {
  selection_tag="$1"
  selection_release_resolution="$2"
  selection_os="$3"
  selection_target="$4"
  selection_target_resolution="$5"
  selection_libc="$6"
  selection_requested_features="$7"
  selection_install_dir="$8"
  selection_service="$9"
  selection_catalog_file="${10}"
  selection_default_asset="${11}"

  selection_default_index=""
  selection_count="$(awk 'END { print NR }' "${selection_catalog_file}")"
  if [ -n "${selection_default_asset}" ]; then
    selection_default_index="$(awk -F '\t' -v asset_name="${selection_default_asset}" '$2 == asset_name { print NR; exit }' "${selection_catalog_file}")"
  fi

  if ! tty_prompt_available; then
    if [ -n "${selection_default_asset}" ]; then
      printf '%s\n' "${selection_default_asset}"
      return 0
    fi
    die "could not resolve a default imagod variant for target ${selection_target}; rerun without -y to choose from the release asset list"
  fi

  {
    printf '%s\n' "Install imagod with the following settings?"
    printf '  tag: %s\n' "${selection_tag}"
    printf '  release_resolution: %s\n' "${selection_release_resolution}"
    printf '  os: %s\n' "${selection_os}"
    printf '  requested_target: %s\n' "${selection_target}"
    printf '  requested_features: %s\n' "$(feature_csv_display "${selection_requested_features}")"
    printf '  target_resolution: %s\n' "${selection_target_resolution}"
    if [ "${selection_target_resolution}" = "auto" ] && [ "${selection_os}" = "linux" ]; then
      printf '  libc: %s\n' "${selection_libc}"
    fi
    printf '  install_dir: %s\n' "${selection_install_dir}"
    printf '  service: %s\n' "${selection_service}"
    printf '%s\n' "Available release variants:"
    awk -F '\t' -v default_asset="${selection_default_asset}" '
      {
        features = ($4 == "" ? "<none>" : $4)
        marker = ($2 == default_asset ? " [default]" : "")
        printf "  %d. %s (target=%s, features=%s)%s\n", NR, $2, $3, features, marker
      }
    ' "${selection_catalog_file}"
  } >/dev/tty

  while :; do
    if [ -n "${selection_default_index}" ]; then
      printf 'Select imagod variant [default: %s, Enter=default, q=cancel] ' "${selection_default_index}" >/dev/tty
    else
      printf 'Select imagod variant [1-%s, q=cancel] ' "${selection_count}" >/dev/tty
    fi

    if ! IFS= read -r selection_reply </dev/tty; then
      selection_reply=""
    fi

    case "${selection_reply}" in
      q|Q|quit|QUIT)
        log "installation cancelled by user"
        exit 0
        ;;
      "")
        if [ -n "${selection_default_asset}" ]; then
          printf '%s\n' "${selection_default_asset}"
          return 0
        fi
        warn "no default variant is available; select an item number"
        ;;
      *)
        case "${selection_reply}" in
          *[!0-9]*)
            warn "invalid selection: ${selection_reply}"
            ;;
          *)
            selected_asset_name="$(awk -F '\t' -v wanted_line="${selection_reply}" 'NR == wanted_line { print $2; exit }' "${selection_catalog_file}")"
            if [ -n "${selected_asset_name}" ]; then
              printf '%s\n' "${selected_asset_name}"
              return 0
            fi
            warn "selection ${selection_reply} is out of range"
            ;;
        esac
        ;;
    esac
  done
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
      die "resolved stable release ${release_resolved_tag} does not provide ${release_asset_name} yet; retry later, use --prerelease, or pass --tag imagod-vX.Y.Z"
      ;;
    prerelease)
      die "resolved prerelease ${release_resolved_tag} does not provide ${release_asset_name} yet; retry later or pass --tag imagod-vX.Y.Z"
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
  install_destination_bin="${install_destination_dir}/imagod"

  if mkdir -p -- "${install_destination_dir}" 2>/dev/null && install -m 0755 -- "${install_source_bin}" "${install_destination_bin}" 2>/dev/null; then
    printf '%s\n' "${install_destination_bin}"
    return 0
  fi

  run_as_root install -d -- "${install_destination_dir}" || die "failed to create install dir: ${install_destination_dir}"
  run_as_root install -m 0755 -- "${install_source_bin}" "${install_destination_bin}" || die "failed to install imagod to ${install_destination_bin}"
  printf '%s\n' "${install_destination_bin}"
}

setup_systemd_service() {
  service_binary="$1"
  systemd_tmp="$(mktemp)"
  cat > "${systemd_tmp}" <<EOF_SYSTEMD
[Unit]
Description=imagod daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${service_binary} --config ${DEFAULT_CONFIG_PATH}
RuntimeDirectory=imago
RuntimeDirectoryMode=0755
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF_SYSTEMD

  if ! run_as_root install -m 0644 -- "${systemd_tmp}" /etc/systemd/system/imagod.service; then
    rm -f "${systemd_tmp}"
    return 1
  fi
  rm -f "${systemd_tmp}"

  run_as_root systemctl daemon-reload &&
    run_as_root systemctl enable --now imagod.service
}

setup_initd_service() {
  service_binary="$1"
  initd_tmp="$(mktemp)"
  cat > "${initd_tmp}" <<EOF_INITD
#!/bin/sh
### BEGIN INIT INFO
# Provides:          imagod
# Required-Start:    \$remote_fs \$network
# Required-Stop:     \$remote_fs \$network
# Default-Start:     2 3 4 5
# Default-Stop:      0 1 6
# Short-Description: imagod daemon
### END INIT INFO

DAEMON='${service_binary}'
DAEMON_ARGS='--config ${DEFAULT_CONFIG_PATH}'
PIDFILE="/var/run/imagod.pid"
NAME="imagod"

start() {
  mkdir -p /run/imago
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

  if ! run_as_root install -m 0755 -- "${initd_tmp}" /etc/init.d/imagod; then
    rm -f "${initd_tmp}"
    return 1
  fi
  rm -f "${initd_tmp}"

  if check_cmd update-rc.d; then
    run_as_root update-rc.d imagod defaults || return 1
  elif check_cmd chkconfig; then
    run_as_root chkconfig --add imagod || return 1
  fi

  if check_cmd service; then
    run_as_root service imagod start
  else
    run_as_root /etc/init.d/imagod start
  fi
}

setup_launchd_system_daemon() {
  service_binary="$1"
  launchd_tmp="$(mktemp)"
  cat > "${launchd_tmp}" <<EOF_LAUNCHD
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>imagod</string>
  <key>ProgramArguments</key>
  <array>
    <string>${service_binary}</string>
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

  if ! run_as_root install -m 0644 -- "${launchd_tmp}" "${LAUNCHD_PLIST_PATH}"; then
    rm -f "${launchd_tmp}"
    return 1
  fi
  rm -f "${launchd_tmp}"

  run_as_root launchctl bootout system "${LAUNCHD_PLIST_PATH}" >/dev/null 2>&1 || true
  run_as_root launchctl bootstrap system "${LAUNCHD_PLIST_PATH}" &&
    run_as_root launchctl enable system/imagod &&
    run_as_root launchctl kickstart -k system/imagod
}

detect_service_manager_or_die() {
  if [ -n "${IMAGOD_TEST_SERVICE_MANAGER:-}" ]; then
    case "${IMAGOD_TEST_SERVICE_MANAGER}" in
      systemd|initd|launchd)
        printf '%s\n' "${IMAGOD_TEST_SERVICE_MANAGER}"
        return 0
        ;;
      none)
        die "no supported init system detected for --with-service"
        ;;
      *)
        die "IMAGOD_TEST_SERVICE_MANAGER must be one of: systemd, initd, launchd, none"
        ;;
    esac
  fi

  if [ "$1" = "darwin" ]; then
    if check_cmd launchctl; then
      printf 'launchd\n'
      return 0
    fi
    die "launchctl not found on macOS; cannot use --with-service"
  fi

  if check_cmd systemctl && [ -d /run/systemd/system ]; then
    printf 'systemd\n'
    return 0
  fi

  if [ -d /etc/init.d ]; then
    printf 'initd\n'
    return 0
  fi

  die "no supported init system detected for --with-service"
}

setup_requested_service() {
  case "$2" in
    systemd)
      setup_systemd_service "$1"
      ;;
    initd)
      setup_initd_service "$1"
      ;;
    launchd)
      setup_launchd_system_daemon "$1"
      ;;
    *)
      return 1
      ;;
  esac
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
      --features)
        [ "$#" -ge 2 ] || die "--features requires a value"
        FEATURES_OVERRIDE="$(normalize_features_csv "$2")"
        FEATURES_EXPLICIT=1
        shift 2
        ;;
      --features=*)
        FEATURES_OVERRIDE="$(normalize_features_csv "${1#--features=}")"
        FEATURES_EXPLICIT=1
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
      --with-service)
        WITH_SERVICE=1
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
  need_cmd sort
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

  if [ -n "${INSTALL_DIR}" ]; then
    install_dir="${INSTALL_DIR}"
  else
    install_dir="$(default_install_dir)"
  fi
  validate_install_dir_path "${install_dir}"

  service_manager=""
  service_status="disabled"
  planned_binary_path="${install_dir}/imagod"
  if [ "${WITH_SERVICE}" = "1" ]; then
    validate_service_binary_path_or_die "${planned_binary_path}"
    service_manager="$(detect_service_manager_or_die "${os}")"
    service_status="enabled (${service_manager})"
  fi

  requested_features="${FEATURES_OVERRIDE}"
  requested_asset_name="$(imagod_asset_name_for_variant "${target}" "${requested_features}")"
  selected_asset_name="${requested_asset_name}"
  selected_target="${target}"
  selected_features="${requested_features}"

  if [ "${DRY_RUN}" != "1" ]; then
    MAIN_TMP_DIR="$(mktemp -d)"
    if [ "${selection_mode}" != "tag" ]; then
      release_metadata_file="${MAIN_TMP_DIR}/release-metadata.json"
      variant_catalog_file="${MAIN_TMP_DIR}/release-variants.tsv"

      download_release_metadata "${resolved_tag}" "${release_metadata_file}"
      build_release_variant_catalog "${release_metadata_file}" "${target}" "${requested_features}" "${variant_catalog_file}"

      default_asset_name=""
      if [ -n "${TARGET_OVERRIDE}" ] || [ "${FEATURES_EXPLICIT}" = "1" ]; then
        if ! catalog_has_asset_name "${variant_catalog_file}" "${requested_asset_name}"; then
          die_with_variant_candidates "${requested_asset_name}" "${resolved_tag}" "${variant_catalog_file}"
        fi
        default_asset_name="${requested_asset_name}"
      else
        auto_asset_name="$(imagod_asset_name_for_variant "${target}" "")"
        if catalog_has_asset_name "${variant_catalog_file}" "${auto_asset_name}"; then
          default_asset_name="${auto_asset_name}"
        fi
      fi

      selected_asset_name="$(select_release_asset_or_exit "${resolved_tag}" "${selection_mode}" "${os}" "${target}" "${target_resolution}" "${libc}" "${requested_features}" "${install_dir}" "${service_status}" "${variant_catalog_file}" "${default_asset_name}")"
      selected_variant="$(parse_imagod_asset_name "${selected_asset_name}")" || die "failed to parse selected imagod asset name: ${selected_asset_name}"
      selected_target="$(printf '%s' "${selected_variant}" | awk -F '\t' 'NR == 1 { print $1 }')"
      selected_features="$(printf '%s' "${selected_variant}" | awk -F '\t' 'NR == 1 { print $2 }')"
    fi
  fi
  checksum_name="${selected_asset_name}.sha256"
  release_url_base="$(resolve_release_url_base "${resolved_tag}")"
  binary_url="${release_url_base}/${selected_asset_name}"
  checksum_url="${release_url_base}/${checksum_name}"

  log "tag: ${resolved_tag}"
  log "release_resolution: ${selection_mode}"
  log "os: ${os}"
  log "target: ${target}"
  log "requested_features: $(feature_csv_display "${requested_features}")"
  log "target_resolution: ${target_resolution}"
  if [ "${target_resolution}" = "auto" ] && [ "${os}" = "linux" ]; then
    log "libc: ${libc}"
  fi
  log "selected_asset: ${selected_asset_name}"
  log "selected_target: ${selected_target}"
  log "selected_features: $(feature_csv_display "${selected_features}")"
  log "install_dir: ${install_dir}"
  log "service: ${service_status}"
  if feature_csv_contains "${selected_features}" "wasi-nn-cvitek"; then
    log "cvitek_runtime: dynamic builds expect CVITEK TPU shared libraries in the system loader path or under ${install_dir}/lib"
  fi

  if [ "${DRY_RUN}" = "1" ]; then
    check_release_assets_for_dry_run "${binary_url}" "${checksum_url}" "${selected_asset_name}" "${checksum_name}" "${selection_mode}" "${resolved_tag}"
    log "dry-run enabled; no changes applied"
    log "selected asset: ${selected_asset_name}"
    log "binary URL: ${binary_url}"
    log "checksum URL: ${checksum_url}"
    exit 0
  fi

  download_release_asset "${binary_url}" "${MAIN_TMP_DIR}/${selected_asset_name}" "${selection_mode}" "${resolved_tag}" "${selected_asset_name}"
  download_release_asset "${checksum_url}" "${MAIN_TMP_DIR}/${checksum_name}" "${selection_mode}" "${resolved_tag}" "${checksum_name}"
  verify_checksum "${MAIN_TMP_DIR}" "${checksum_name}" "${selected_asset_name}"

  installed_bin="$(install_binary "${MAIN_TMP_DIR}/${selected_asset_name}" "${install_dir}")"
  log "installed binary: ${installed_bin}"

  if [ "${WITH_SERVICE}" = "1" ]; then
    if ! setup_requested_service "${installed_bin}" "${service_manager}"; then
      die "binary installed at ${installed_bin}, but service setup failed"
    fi
    log "service installed and started: ${service_manager}"
  fi

  if ! path_contains "${install_dir}"; then
    log "PATH hint: add ${install_dir} to PATH"
  fi

  log "imagod installation completed"
  log "run '${installed_bin} --help' to verify installation"
}

main "$@"
