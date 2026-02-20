#!/usr/bin/env bash

resolve_paths() {
  local script_path="${1:-${BASH_SOURCE[1]:-}}"
  if [[ -z "${script_path}" ]]; then
    echo "error: resolve_paths requires the caller script path" >&2
    return 1
  fi

  ROOT_DIR="$(cd -- "$(dirname -- "${script_path}")/.." && pwd)"
  REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"
  export ROOT_DIR
  export REPO_ROOT
}

run_imago_cli() {
  if [[ -z "${ROOT_DIR:-}" || -z "${REPO_ROOT:-}" ]]; then
    echo "error: ROOT_DIR/REPO_ROOT is not set; call resolve_paths first" >&2
    return 1
  fi
  if [[ $# -eq 0 ]]; then
    echo "error: run_imago_cli requires at least one argument" >&2
    return 1
  fi

  (
    cd "${ROOT_DIR}"
    cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imago-cli -- "$@"
  )
}

run_imagod() {
  if [[ -z "${ROOT_DIR:-}" || -z "${REPO_ROOT:-}" ]]; then
    echo "error: ROOT_DIR/REPO_ROOT is not set; call resolve_paths first" >&2
    return 1
  fi

  (
    cd "${ROOT_DIR}"
    cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imagod -- --config "${ROOT_DIR}/imagod.toml" "$@"
  )
}

remove_known_host_entry() {
  local authority="$1"
  local known_hosts_path="${HOME}/.imago/known_hosts"

  if [[ ! -f "${known_hosts_path}" ]]; then
    return 0
  fi

  local tmp_path
  tmp_path="$(mktemp "${known_hosts_path}.tmp.XXXXXX")"
  awk -F '\t' -v authority="${authority}" '$1 != authority { print }' "${known_hosts_path}" > "${tmp_path}"
  mv "${tmp_path}" "${known_hosts_path}"
  chmod 600 "${known_hosts_path}" 2>/dev/null || true
}

reset_local_known_hosts() {
  remove_known_host_entry "localhost:4443"
  remove_known_host_entry "127.0.0.1:4443"
}

deploy_default() {
  reset_local_known_hosts
  run_imago_cli deploy --target default "$@"
}
