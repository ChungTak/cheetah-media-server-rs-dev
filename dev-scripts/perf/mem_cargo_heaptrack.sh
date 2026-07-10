#!/usr/bin/env bash
# Heap allocation trace with cargo-heaptrack on a crate's unit tests.
# Usage: ./dev-scripts/perf/mem_cargo_heaptrack.sh [crate]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

CRATE="${1:-cheetah-codec}"
OUT_DIR="${OUT_DIR:-target/perf}"
mkdir -p "${OUT_DIR}"

if ! cargo heaptrack --help >/dev/null 2>&1; then
  echo "[mem_cargo_heaptrack] cargo-heaptrack not found; install with: cargo install cargo-heaptrack" >&2
  exit 1
fi

echo "[mem_cargo_heaptrack] running heaptrack on ${CRATE} unit tests"
cargo heaptrack --unit-test "${CRATE}" -o "${OUT_DIR}/heaptrack-${CRATE}" -- --test-threads=1

echo "[mem_cargo_heaptrack] done: output in ${OUT_DIR}/heaptrack-${CRATE}.*"
