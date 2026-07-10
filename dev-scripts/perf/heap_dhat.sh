#!/usr/bin/env bash
# Heap allocation profile using dhat (dhat-rs) on the `dhat_alloc` benchmark.
# Usage: ./dev-scripts/perf/heap_dhat.sh [crate]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

CRATE="${1:-cheetah-codec}"
OUT_DIR="${OUT_DIR:-target/perf}"
mkdir -p "${OUT_DIR}"

echo "[heap_dhat] running dhat_alloc benchmark for ${CRATE}"
cargo bench -p "${CRATE}" --bench dhat_alloc

# dhat writes dhat-heap.json to the current working directory of the bench
# binary; for workspace members that is typically the crate root.
DHAT_FILE=$(find "${ROOT_DIR}" -maxdepth 4 -name 'dhat-heap.json' -mmin -1 -type f -print -quit || true)
if [ -n "${DHAT_FILE}" ] && [ -f "${DHAT_FILE}" ]; then
  mv "${DHAT_FILE}" "${OUT_DIR}/dhat-${CRATE}-heap.json"
  echo "[heap_dhat] done: ${OUT_DIR}/dhat-${CRATE}-heap.json"
else
  echo "[heap_dhat] warning: dhat-heap.json not found" >&2
fi
