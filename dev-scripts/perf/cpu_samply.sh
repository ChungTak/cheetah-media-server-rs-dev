#!/usr/bin/env bash
# Profile CPU hotspots of a crate's unit-test binary using `samply`.
# Usage: ./dev-scripts/perf/cpu_samply.sh [crate] [filter]
#   crate   - package name (default: cheetah-codec)
#   filter  - optional test filter passed to the unit-test binary
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

CRATE="${1:-cheetah-codec}"
FILTER="${2:-}"
OUT_DIR="${OUT_DIR:-target/perf}"
mkdir -p "${OUT_DIR}"

SAMPLY="$(command -v samply || echo "${HOME}/.cargo/bin/samply")"
if [ ! -x "${SAMPLY}" ]; then
  echo "[cpu_samply] samply not found; install with: cargo install samply" >&2
  exit 1
fi

echo "[cpu_samply] building unit tests for ${CRATE}"
cargo test -p "${CRATE}" --lib --no-run --message-format=short 2>&1 \
  | tee "${OUT_DIR}/samply-${CRATE}-build.log" \
  | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' > "${OUT_DIR}/samply-${CRATE}-bin.txt"

BIN_PATH="$(tail -n 1 "${OUT_DIR}/samply-${CRATE}-bin.txt")"
if [ -z "${BIN_PATH}" ] || [ ! -x "${BIN_PATH}" ]; then
  echo "[cpu_samply] could not locate unit-test binary" >&2
  exit 1
fi

OUT_FILE="${OUT_DIR}/samply-${CRATE}.json"
echo "[cpu_samply] profiling ${BIN_PATH} -> ${OUT_FILE}"
if [ -n "${FILTER}" ]; then
  sudo -E "${SAMPLY}" record -n -s -o "${OUT_FILE}" -- "${BIN_PATH}" --test-threads=1 "${FILTER}"
else
  sudo -E "${SAMPLY}" record -n -s -o "${OUT_FILE}" -- "${BIN_PATH}" --test-threads=1
fi

echo "[cpu_samply] done: ${OUT_FILE}"
echo "[cpu_samply] view with: samply load ${OUT_FILE}"
