#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  ./scripts/measure_runner_memory.sh <example-dir>

Examples:
  ./scripts/measure_runner_memory.sh examples/local-imagod
  ./scripts/measure_runner_memory.sh examples/local-imagod-http
EOF
}

if [[ $# -ne 1 ]]; then
  usage
  exit 1
fi

example_dir="$1"
if [[ ! -d "$example_dir" ]]; then
  echo "error: example directory not found: $example_dir" >&2
  exit 1
fi
if [[ ! -f "$example_dir/imagod.toml" ]]; then
  echo "error: imagod.toml not found under: $example_dir" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo command not found" >&2
  exit 1
fi

example_abs="$(cd "$example_dir" && pwd)"
workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
original_home="${HOME:-}"

tmp_home="$(mktemp -d "${TMPDIR:-/tmp}/imago-memory-home.XXXXXX")"
tmp_work="$(mktemp -d "${TMPDIR:-/tmp}/imago-memory-work.XXXXXX")"
daemon_log="$tmp_work/imagod.log"
deploy_log="$tmp_work/deploy.log"
runner_pid=""
daemon_pid=""
manager_pid=""
ready_timeout_secs="${IMAGOD_MEASURE_READY_TIMEOUT_SECS:-900}"

cleanup() {
  set +e
  if [[ -n "$manager_pid" ]] && kill -0 "$manager_pid" >/dev/null 2>&1; then
    kill "$manager_pid" >/dev/null 2>&1 || true
    wait "$manager_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" >/dev/null 2>&1; then
    kill "$daemon_pid" >/dev/null 2>&1 || true
    wait "$daemon_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_home" "$tmp_work"
}
trap cleanup EXIT INT TERM

if [[ -z "${RUSTUP_HOME:-}" ]] && [[ -n "$original_home" ]] && [[ -d "$original_home/.rustup" ]]; then
  export RUSTUP_HOME="$original_home/.rustup"
fi
if [[ -z "${CARGO_HOME:-}" ]] && [[ -n "$original_home" ]] && [[ -d "$original_home/.cargo" ]]; then
  export CARGO_HOME="$original_home/.cargo"
fi

export HOME="$tmp_home"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_CACHE_HOME="$HOME/.cache"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"

echo "== Runner Memory Measurement =="
echo "example_dir: $example_dir"
echo "build_profile: release"
echo "isolated_home: $HOME"

(
  cd "$example_abs"
  cargo run --release -p imagod -- --config imagod.toml
) >"$daemon_log" 2>&1 &
daemon_pid="$!"
echo "imagod_pid: $daemon_pid"

ready_deadline=$((SECONDS + ready_timeout_secs))
while true; do
  if ! kill -0 "$daemon_pid" >/dev/null 2>&1; then
    echo "error: imagod exited before ready" >&2
    tail -n 200 "$daemon_log" >&2 || true
    exit 1
  fi
  if grep -q "imagod listening on" "$daemon_log"; then
    break
  fi
  if (( SECONDS >= ready_deadline )); then
    break
  fi
  sleep 0.25
done

if ! grep -q "imagod listening on" "$daemon_log"; then
  echo "error: imagod did not become ready within ${ready_timeout_secs}s timeout" >&2
  tail -n 200 "$daemon_log" >&2 || true
  exit 1
fi

manager_pid="$(
  awk -v cmd="$workspace_root/target/release/imagod --config imagod.toml" \
    'index($0, cmd) { print $1; exit }' \
    < <(ps -axo pid=,command=)
)"
if [[ -z "$manager_pid" ]]; then
  manager_pid="$daemon_pid"
fi
echo "manager_pid: $manager_pid"

(
  cd "$example_abs"
  cargo run --release -p imago-cli -- service deploy --target default --detach
) >"$deploy_log" 2>&1 || {
  echo "error: deploy command failed" >&2
  tail -n 200 "$deploy_log" >&2 || true
  exit 1
}

for _ in $(seq 1 160); do
  if ! kill -0 "$manager_pid" >/dev/null 2>&1; then
    echo "error: imagod exited before runner detection" >&2
    tail -n 200 "$daemon_log" >&2 || true
    exit 1
  fi
  runner_pid="$(
    awk -v parent_pid="$manager_pid" '$2 == parent_pid && index($0, "--runner") { print $1; exit }' \
      < <(ps -axo pid=,ppid=,command=)
  )"
  if [[ -z "$runner_pid" ]]; then
    runner_pid="$(
      awk -v cmd="$workspace_root/target/release/imagod --runner" \
        'index($0, cmd) { pid = $1 } END { if (pid != "") print pid }' \
        < <(ps -axo pid=,command=)
    )"
  fi
  if [[ -n "$runner_pid" ]]; then
    break
  fi
  sleep 0.25
done

if [[ -z "$runner_pid" ]]; then
  echo "error: runner process was not detected" >&2
  tail -n 200 "$daemon_log" >&2 || true
  tail -n 200 "$deploy_log" >&2 || true
  exit 1
fi

rss_samples=()
runner_exited_during_sampling=0
for _ in $(seq 1 8); do
  if ! kill -0 "$runner_pid" >/dev/null 2>&1; then
    runner_exited_during_sampling=1
    break
  fi
  rss_kb="$(ps -o rss= -p "$runner_pid" | tr -d '[:space:]')"
  if [[ -n "$rss_kb" ]]; then
    rss_samples+=("$rss_kb")
  fi
  sleep 0.25
done

if [[ ${#rss_samples[@]} -eq 0 ]]; then
  echo "error: failed to collect RSS samples (runner exited too quickly)" >&2
  tail -n 200 "$daemon_log" >&2 || true
  exit 1
fi

thread_count="$(ps -o thcount= -p "$runner_pid" 2>/dev/null | tr -d '[:space:]' || true)"
if [[ ! "$thread_count" =~ ^[0-9]+$ ]]; then
  thread_count="$(ps -M -p "$runner_pid" 2>/dev/null | tail -n +2 | wc -l | tr -d '[:space:]' || true)"
fi
if [[ ! "$thread_count" =~ ^[0-9]+$ ]]; then
  thread_count="n/a"
fi

rss_stats="$(
  printf '%s\n' "${rss_samples[@]}" \
    | awk '
      NR == 1 { min = $1; max = $1; sum = $1; next }
      { if ($1 < min) min = $1; if ($1 > max) max = $1; sum += $1 }
      END { printf("samples=%d min_kb=%d avg_kb=%.0f max_kb=%d", NR, min, sum / NR, max) }
    '
)"

echo "runner_pid: $runner_pid"
echo "rss_samples_kb: ${rss_samples[*]}"
echo "rss_stats: $rss_stats"
if [[ "$runner_exited_during_sampling" -eq 1 ]]; then
  echo "sampling_note: runner exited before full sample window; reported with partial samples"
fi
echo "thread_count: $thread_count"

echo "vmmap_summary:"
if command -v vmmap >/dev/null 2>&1; then
  vmmap_output="$(vmmap -summary "$runner_pid" 2>/dev/null || true)"
  if [[ -n "$vmmap_output" ]]; then
    echo "$vmmap_output" | grep -E 'Physical footprint|MALLOC_SMALL|Stack|VM_ALLOCATE|TOTAL' || true
  else
    echo "vmmap output unavailable"
  fi
else
  echo "vmmap command not found"
fi

echo "done"
