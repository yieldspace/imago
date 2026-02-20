#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../_shared" && pwd)/common.sh"

resolve_paths "${BASH_SOURCE[0]}"
run_imagod
