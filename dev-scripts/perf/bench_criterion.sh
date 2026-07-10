#!/usr/bin/env bash
# Micro-benchmarks using Criterion for a crate.
# Usage: ./dev-scripts/perf/bench_criterion.sh [crate] [bench-name]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

CRATE="${1:-cheetah-codec}"
BENCH="${2:-}"
OUT_DIR="${OUT_DIR:-target/perf}"
mkdir -p "${OUT_DIR}"

if [ -n "${BENCH}" ]; then
  echo "[bench_criterion] running bench ${BENCH} for ${CRATE}"
  cargo bench -p "${CRATE}" --bench "${BENCH}"
else
  echo "[bench_criterion] running all benches for ${CRATE}"
  cargo bench -p "${CRATE}"
fi

echo "[bench_criterion] done: reports in target/criterion/"
