#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"
KNOWN_HOSTS_PATH="${HOME}/.imago/known_hosts"
IMAGOD_LOG_PATH="${ROOT_DIR}/.imagod-e2e.log"
IMAGOD_PID=""

run_imago_cli() {
  cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imago-cli -- "$@"
}

require_raw_ed25519_hex() {
  local key_hex="$1"
  local label="$2"
  if [[ "${#key_hex}" -ne 64 ]] || [[ ! "${key_hex}" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "error: ${label} must be 64-hex raw ed25519 key: ${key_hex}" >&2
    exit 1
  fi
}

cleanup() {
  local exit_code=$?
  if [[ -n "${IMAGOD_PID}" ]] && kill -0 "${IMAGOD_PID}" 2>/dev/null; then
    kill "${IMAGOD_PID}" 2>/dev/null || true
    wait "${IMAGOD_PID}" 2>/dev/null || true
  fi
  return "${exit_code}"
}
trap cleanup EXIT

update_imagod_tls_keys() {
  local admin_key_hex="$1"
  local rpc_key_hex="$2"
  perl -0pi -e \
    "s/admin_public_keys = \\[[^\\]]*\\]/admin_public_keys = [\\\"${admin_key_hex}\\\"]/g; s/client_public_keys = \\[[^\\]]*\\]/client_public_keys = [\\\"${rpc_key_hex}\\\"]/g" \
    "${ROOT_DIR}/imagod.toml"
}

remove_known_host_entry() {
  local authority="$1"
  if [[ ! -f "${KNOWN_HOSTS_PATH}" ]]; then
    return 0
  fi

  local tmp_path
  tmp_path="$(mktemp "${KNOWN_HOSTS_PATH}.tmp.XXXXXX")"
  awk -F '\t' -v authority="${authority}" '$1 != authority { print }' "${KNOWN_HOSTS_PATH}" > "${tmp_path}"
  mv "${tmp_path}" "${KNOWN_HOSTS_PATH}"
  chmod 600 "${KNOWN_HOSTS_PATH}" 2>/dev/null || true
}

wait_for_imagod() {
  sleep 5
}

stop_existing_example_imagod() {
  if ! command -v pgrep >/dev/null 2>&1; then
    return 0
  fi

  local pattern
  pattern="target/debug/imagod --config ${ROOT_DIR}/imagod.toml"
  local pids
  pids="$(pgrep -f "${pattern}" || true)"
  if [[ -z "${pids}" ]]; then
    return 0
  fi

  echo "stopping existing imagod process for this example: ${pids}" >&2
  while IFS= read -r pid; do
    if [[ -n "${pid}" ]]; then
      kill "${pid}" 2>/dev/null || true
      wait "${pid}" 2>/dev/null || true
    fi
  done <<< "${pids}"
}

ensure_imagod_port_available() {
  if (exec 3<>"/dev/tcp/127.0.0.1/4443") 2>/dev/null; then
    exec 3>&-
    exec 3<&-
    echo "error: 127.0.0.1:4443 is already in use; stop the existing imagod and retry" >&2
    exit 1
  fi
}

collect_cli_client_logs() {
  local log_output=""
  local retries=40
  while (( retries > 0 )); do
    log_output="$(
      cd "${ROOT_DIR}" &&
        run_imago_cli compose logs dev --target default --name cli-client --tail 200 2>&1 || true
    )"
    if echo "${log_output}" | rg -q "acme:clock/api.now =>"; then
      printf '%s\n' "${log_output}"
      return 0
    fi
    sleep 2
    retries=$((retries - 1))
  done

  echo "error: timeout waiting for cli-client rpc log output" >&2
  printf '%s\n' "${log_output}" >&2
  return 1
}

run_compose_deploy_with_retry() {
  local output=""
  local retries=4

  while (( retries > 0 )); do
    if output="$(cd "${ROOT_DIR}" && run_imago_cli compose deploy dev --target default 2>&1)"; then
      printf '%s\n' "${output}"
      return 0
    fi
    retries=$((retries - 1))
    if (( retries == 0 )); then
      printf '%s\n' "${output}" >&2
      return 1
    fi
    sleep 2
  done

  return 1
}

main() {
  mkdir -p \
    "${ROOT_DIR}/certs" \
    "${ROOT_DIR}/certs/rpc"

  echo "[1/9] generate admin/server cert material"
  run_imago_cli certs generate --out-dir "${ROOT_DIR}/certs" --force

  echo "[2/9] generate rpc-client cert material"
  run_imago_cli certs generate --out-dir "${ROOT_DIR}/certs/rpc" --force

  local admin_pub_hex
  local rpc_pub_hex
  admin_pub_hex="$(tr -d '[:space:]' < "${ROOT_DIR}/certs/client.pub.hex")"
  rpc_pub_hex="$(tr -d '[:space:]' < "${ROOT_DIR}/certs/rpc/client.pub.hex")"
  require_raw_ed25519_hex "${admin_pub_hex}" "admin public key"
  require_raw_ed25519_hex "${rpc_pub_hex}" "rpc public key"

  if [[ "${admin_pub_hex}" == "${rpc_pub_hex}" ]]; then
    echo "error: admin/client public keys must differ" >&2
    exit 1
  fi

  update_imagod_tls_keys "${admin_pub_hex}" "${rpc_pub_hex}"

  echo "[3/9] reset localhost TOFU pin"
  remove_known_host_entry "localhost:4443"

  echo "[4/9] prebuild rpc-greeter for component source"
  (
    cd "${ROOT_DIR}"
    run_imago_cli compose build prepare --target default
  )

  echo "[5/9] run compose update"
  (
    cd "${ROOT_DIR}"
    run_imago_cli compose update dev
  )

  echo "[6/9] run compose build"
  (
    cd "${ROOT_DIR}"
    run_imago_cli compose build dev --target default
  )

  echo "[7/9] start imagod"
  stop_existing_example_imagod
  ensure_imagod_port_available
  : > "${IMAGOD_LOG_PATH}"
  (
    cd "${ROOT_DIR}"
    cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imagod -- --config "${ROOT_DIR}/imagod.toml"
  ) >"${IMAGOD_LOG_PATH}" 2>&1 &
  IMAGOD_PID=$!
  wait_for_imagod
  if ! kill -0 "${IMAGOD_PID}" 2>/dev/null; then
    echo "error: imagod failed to start" >&2
    cat "${IMAGOD_LOG_PATH}" >&2 || true
    exit 1
  fi

  echo "[8/9] compose deploy"
  if ! run_compose_deploy_with_retry; then
    echo "error: compose deploy failed" >&2
    (
      cd "${ROOT_DIR}"
      run_imago_cli compose logs dev --target default --name cli-client --tail 200 || true
    ) >&2
    exit 1
  fi

  echo "[9/9] verify cli-client logs"
  collect_cli_client_logs

  echo "ok: imago-compose-bindings e2e succeeded"
}

main "$@"
