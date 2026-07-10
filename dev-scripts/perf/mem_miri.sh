#!/usr/bin/env bash
# Miri UB / memory-safety check for a crate's unit tests.
# Usage: ./dev-scripts/perf/mem_miri.sh [crate]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

CRATE="${1:-cheetah-codec}"
OUT_DIR="${OUT_DIR:-target/perf}"
mkdir -p "${OUT_DIR}"

if ! rustup run nightly cargo miri --version >/dev/null 2>&1; then
  echo "[mem_miri] Miri not found on nightly; install with:" >&2
  echo "  rustup toolchain install nightly" >&2
  echo "  rustup component add miri --toolchain nightly" >&2
  exit 1
fi

echo "[mem_miri] running miri on ${CRATE} --lib"
MIRI_LOG=warn \
  rustup run nightly cargo miri test -p "${CRATE}" --lib -- --test-threads=1 \
  2>&1 | tee "${OUT_DIR}/miri-${CRATE}.log"

echo "[mem_miri] done: ${OUT_DIR}/miri-${CRATE}.log"
