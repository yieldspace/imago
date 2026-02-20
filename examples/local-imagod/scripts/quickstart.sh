#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUN_SCRIPT="${SCRIPT_DIR}/run-imagod.sh"
DEPLOY_SCRIPT="${SCRIPT_DIR}/deploy.sh"
VERIFY_SCRIPT="${SCRIPT_DIR}/verify.sh"
MANAGER_SOCKET_PATH="${SCRIPT_DIR}/../.imagod-data/runtime/ipc/manager-control.sock"
TAIL_LINES="${1:-200}"
READY_TIMEOUT_SECS="${IMAGOD_READY_TIMEOUT_SECS:-30}"
IMAGOD_PID=""

kill_process_tree() {
  local signal="$1"
  local pid="$2"
  local children=()
  local child

  while IFS= read -r child; do
    [[ -n "${child}" ]] || continue
    children+=("${child}")
  done < <(pgrep -P "${pid}" || true)

  if ((${#children[@]} > 0)); then
    for child in "${children[@]}"; do
      kill_process_tree "${signal}" "${child}"
    done
  fi

  kill "-${signal}" "${pid}" 2>/dev/null || true
}

cleanup() {
  local exit_code=$?
  trap - EXIT
  if [[ -n "${IMAGOD_PID}" ]] && kill -0 "${IMAGOD_PID}" 2>/dev/null; then
    kill_process_tree TERM "${IMAGOD_PID}"
    sleep 1
    if kill -0 "${IMAGOD_PID}" 2>/dev/null; then
      kill_process_tree KILL "${IMAGOD_PID}"
    fi
    wait "${IMAGOD_PID}" 2>/dev/null || true
  fi
  exit "${exit_code}"
}
trap cleanup EXIT

if ! [[ "${TAIL_LINES}" =~ ^[0-9]+$ ]] || [[ "${TAIL_LINES}" -le 0 ]]; then
  echo "error: verify tail must be a positive integer: ${TAIL_LINES}" >&2
  exit 1
fi

if ! [[ "${READY_TIMEOUT_SECS}" =~ ^[0-9]+$ ]]; then
  echo "error: IMAGOD_READY_TIMEOUT_SECS must be a non-negative integer: ${READY_TIMEOUT_SECS}" >&2
  exit 1
fi

"${RUN_SCRIPT}" &
IMAGOD_PID="$!"

ready_wait_started_at="${SECONDS}"
while true; do
  if ! kill -0 "${IMAGOD_PID}" 2>/dev/null; then
    wait "${IMAGOD_PID}" || true
    echo "error: imagod exited before manager-control.sock became ready" >&2
    exit 1
  fi

  if [[ -S "${MANAGER_SOCKET_PATH}" ]]; then
    break
  fi

  ready_wait_elapsed=$((SECONDS - ready_wait_started_at))
  if [[ "${ready_wait_elapsed}" -ge "${READY_TIMEOUT_SECS}" ]]; then
    echo "error: timed out waiting for manager-control.sock (${MANAGER_SOCKET_PATH}) after ${READY_TIMEOUT_SECS}s" >&2
    exit 1
  fi

  sleep 1
done

"${DEPLOY_SCRIPT}"
"${VERIFY_SCRIPT}" "${TAIL_LINES}"
