#!/usr/bin/env bash
# CPU profiling with cargo-flamegraph. Falls back to `samply` if `perf` cannot run.
# Usage: ./dev-scripts/perf/cpu_flamegraph.sh [crate]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

CRATE="${1:-cheetah-codec}"

if perf record -F 99 -g -- sleep 0.1 >/dev/null 2>&1; then
  echo "[cpu_flamegraph] perf is available; running cargo flamegraph"
  cargo flamegraph --unit-test -p "${CRATE}" -- --test-threads=1
else
  echo "[cpu_flamegraph] perf cannot run in this environment; falling back to cpu_samply.sh"
  "${ROOT_DIR}/dev-scripts/perf/cpu_samply.sh" "${CRATE}"
fi
