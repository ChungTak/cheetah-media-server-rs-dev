#!/usr/bin/env bash
# Memory-leak / invalid-memory check with valgrind on a crate's unit-test binary.
# Usage: ./dev-scripts/perf/mem_valgrind.sh [crate]
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT_DIR}"

CRATE="${1:-cheetah-codec}"
OUT_DIR="${OUT_DIR:-target/perf}"
mkdir -p "${OUT_DIR}"

if ! command -v valgrind >/dev/null 2>&1; then
  echo "[mem_valgrind] valgrind not found; install with: apt install valgrind" >&2
  exit 1
fi

echo "[mem_valgrind] building unit tests for ${CRATE}"
cargo test -p "${CRATE}" --lib --no-run --message-format=short 2>&1 \
  | tee "${OUT_DIR}/valgrind-${CRATE}-build.log" \
  | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' > "${OUT_DIR}/valgrind-${CRATE}-bin.txt"

BIN_PATH="$(tail -n 1 "${OUT_DIR}/valgrind-${CRATE}-bin.txt")"
if [ -z "${BIN_PATH}" ] || [ ! -x "${BIN_PATH}" ]; then
  echo "[mem_valgrind] could not locate unit-test binary" >&2
  exit 1
fi

OUT_FILE="${OUT_DIR}/valgrind-${CRATE}.log"
echo "[mem_valgrind] running valgrind on ${BIN_PATH}"
valgrind \
  --error-exitcode=1 \
  --leak-check=full \
  --errors-for-leak-kinds=definite \
  --show-leak-kinds=definite,reachable \
  --log-file="${OUT_FILE}" \
  "${BIN_PATH}" --test-threads=1

echo "[mem_valgrind] done: ${OUT_FILE}"
