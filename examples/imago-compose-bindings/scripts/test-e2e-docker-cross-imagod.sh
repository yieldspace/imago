#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd -- "${ROOT_DIR}/../.." && pwd)"
DOCKER_DIR="${ROOT_DIR}/docker"
DOCKER_COMPOSE_FILE="${DOCKER_DIR}/docker-compose.yml"
DOCKER_PROJECT_NAME="imago-compose-bindings-alice-bob-e2e"
DEPLOYER_WORKDIR="/workspace/examples/imago-compose-bindings/docker"

run_imago_cli_host() {
  cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p imago-cli -- "$@"
}

docker_compose() {
  docker compose \
    --project-directory "${DOCKER_DIR}" \
    -f "${DOCKER_COMPOSE_FILE}" \
    --project-name "${DOCKER_PROJECT_NAME}" \
    "$@"
}

docker_exec_deployer() {
  docker_compose exec -T --workdir "${DEPLOYER_WORKDIR}" imago-deployer "$@"
}

prepare_imago_cli_in_deployer() {
  docker_exec_deployer cargo build --manifest-path /workspace/Cargo.toml -p imago-cli --release
}

run_imago_cli_in_deployer() {
  docker_exec_deployer /workspace/target/release/imago "$@"
}

require_raw_ed25519_hex() {
  local key_hex="$1"
  local label="$2"
  if [[ "${#key_hex}" -ne 64 ]] || [[ ! "${key_hex}" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "error: ${label} must be 64-hex raw ed25519 key: ${key_hex}" >&2
    exit 1
  fi
}

update_imagod_tls_keys() {
  local admin_key_hex="$1"
  local alice_client_key_hex="$2"
  local bob_client_key_hex="$3"

  perl -0pi -e \
    "s/admin_public_keys = \\[[^\\]]*\\]/admin_public_keys = [\\\"${admin_key_hex}\\\"]/g; s/client_public_keys = \\[[^\\]]*\\]/client_public_keys = [\\\"${alice_client_key_hex}\\\"]/g" \
    "${DOCKER_DIR}/imagod-alice.toml"

  perl -0pi -e \
    "s/admin_public_keys = \\[[^\\]]*\\]/admin_public_keys = [\\\"${admin_key_hex}\\\"]/g; s/client_public_keys = \\[[^\\]]*\\]/client_public_keys = [\\\"${bob_client_key_hex}\\\"]/g" \
    "${DOCKER_DIR}/imagod-bob.toml"
}

wait_for_port_in_deployer() {
  local host="$1"
  local port="$2"
  local retries=45

  while (( retries > 0 )); do
    if docker_exec_deployer bash -lc "exec 3<>/dev/tcp/${host}/${port}; exec 3>&-; exec 3<&-" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
    retries=$((retries - 1))
  done

  echo "error: timeout waiting for ${host}:${port} from imago-deployer" >&2
  return 1
}

seed_deployer_known_hosts() {
  local alice_pub_hex="$1"
  local bob_pub_hex="$2"

  docker_exec_deployer bash -lc "set -euo pipefail; mkdir -p /root/.imago; printf 'imagod-alice:4443\\t%s\\nimagod-bob:4443\\t%s\\n' '${alice_pub_hex}' '${bob_pub_hex}' > /root/.imago/known_hosts; chmod 600 /root/.imago/known_hosts"
}

run_compose_deploy_with_retry() {
  local profile="$1"
  local target="$2"
  local output=""
  local retries=4

  while (( retries > 0 )); do
    if output="$(run_imago_cli_in_deployer compose deploy "${profile}" --target "${target}" 2>&1)"; then
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

run_profile_update_build_deploy() {
  local profile="$1"
  local target="$2"

  run_imago_cli_in_deployer compose update "${profile}"
  run_imago_cli_in_deployer compose build "${profile}" --target "${target}"

  if ! run_compose_deploy_with_retry "${profile}" "${target}"; then
    echo "error: compose deploy failed for profile '${profile}'" >&2
    run_imago_cli_in_deployer compose logs "${profile}" --target "${target}" --tail 200 >&2 || true
    exit 1
  fi
}

collect_cli_client_logs_with_pattern() {
  local pattern="$1"
  local label="$2"
  local output=""
  local retries=40

  while (( retries > 0 )); do
    output="$(run_imago_cli_in_deployer compose logs client --target alice --name cli-client --tail 200 2>&1 || true)"
    if echo "${output}" | rg -q -e "${pattern}"; then
      printf '%s\n' "${output}"
      return 0
    fi
    sleep 2
    retries=$((retries - 1))
  done

  echo "error: timeout waiting for '${label}' log pattern" >&2
  printf '%s\n' "${output}" >&2
  return 1
}

cleanup() {
  local exit_code=$?
  if [[ -f "${DOCKER_COMPOSE_FILE}" ]]; then
    docker_compose down --remove-orphans >/dev/null 2>&1 || true
  fi
  return "${exit_code}"
}
trap cleanup EXIT

main() {
  if ! command -v docker >/dev/null 2>&1; then
    echo "error: docker command is required" >&2
    exit 1
  fi

  if ! docker compose version >/dev/null 2>&1; then
    echo "error: docker compose v2 is required" >&2
    exit 1
  fi

  mkdir -p \
    "${DOCKER_DIR}/certs/control" \
    "${DOCKER_DIR}/certs/alice" \
    "${DOCKER_DIR}/certs/bob"

  echo "[1/11] generate control/alice/bob cert material"
  run_imago_cli_host certs generate --out-dir "${DOCKER_DIR}/certs/control" --force
  run_imago_cli_host certs generate --out-dir "${DOCKER_DIR}/certs/alice" --server-name imagod-alice --force
  run_imago_cli_host certs generate --out-dir "${DOCKER_DIR}/certs/bob" --server-name imagod-bob --force

  local control_admin_pub_hex
  local alice_server_pub_hex
  local bob_server_pub_hex
  control_admin_pub_hex="$(tr -d '[:space:]' < "${DOCKER_DIR}/certs/control/client.pub.hex")"
  alice_server_pub_hex="$(tr -d '[:space:]' < "${DOCKER_DIR}/certs/alice/server.pub.hex")"
  bob_server_pub_hex="$(tr -d '[:space:]' < "${DOCKER_DIR}/certs/bob/server.pub.hex")"

  require_raw_ed25519_hex "${control_admin_pub_hex}" "control admin public key"
  require_raw_ed25519_hex "${alice_server_pub_hex}" "alice server public key"
  require_raw_ed25519_hex "${bob_server_pub_hex}" "bob server public key"

  update_imagod_tls_keys "${control_admin_pub_hex}" "${alice_server_pub_hex}" "${bob_server_pub_hex}"

  echo "[2/11] docker compose up --build -d (alice/bob/deployer)"
  docker_compose down --remove-orphans >/dev/null 2>&1 || true
  docker_compose up --build -d imagod-alice imagod-bob imago-deployer

  echo "[3/11] wait for imagod ports from imago-deployer"
  wait_for_port_in_deployer "imagod-alice" 4443
  wait_for_port_in_deployer "imagod-bob" 4443

  echo "[4/11] build imago-cli in imago-deployer"
  prepare_imago_cli_in_deployer

  echo "[5/11] seed known_hosts in imago-deployer"
  seed_deployer_known_hosts "${alice_server_pub_hex}" "${bob_server_pub_hex}"

  echo "[6/11] compose update/build/deploy greeter -> bob (inside imago-deployer)"
  run_profile_update_build_deploy "greeter" "bob"

  echo "[7/11] compose update/build/deploy client -> alice (inside imago-deployer)"
  run_profile_update_build_deploy "client" "alice"

  echo "[8/11] verify pre-cert failure logs"
  collect_cli_client_logs_with_pattern "imago:node/rpc connection failed:|acme:clock/api.now failed:" "pre-cert failure"

  echo "[9/11] deploy binding cert (alice -> bob) from imago-deployer"
  run_imago_cli_in_deployer bindings cert deploy --from imagod-alice:4443 --to imagod-bob:4443

  echo "[10/11] verify success logs"
  collect_cli_client_logs_with_pattern "acme:clock/api.now =>" "post-cert success"

  echo "[11/11] ok: imago-compose-bindings docker 2-node e2e succeeded"
}

main "$@"
