#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

echo "[fuzz-smoke] checking cheetah-rtmp-fuzz fuzz targets compile"
cargo check --manifest-path crates/protocols/rtmp/fuzz/Cargo.toml --bins

echo "[fuzz-smoke] all fuzz targets compile"

RUN_MODE="${RUN_FUZZ_SMOKE_RUN:-0}"
if [[ "${RUN_MODE}" != "1" ]]; then
  echo "[fuzz-smoke] runtime fuzz smoke disabled (set RUN_FUZZ_SMOKE_RUN=1 to enable)"
  exit 0
fi

MAX_TOTAL_TIME="${FUZZ_MAX_TOTAL_TIME:-3}"

if [[ -n "${FUZZ_TARGETS:-}" ]]; then
  read -r -a TARGETS <<<"${FUZZ_TARGETS}"
else
  TARGETS=()
  shopt -s nullglob
  for f in crates/protocols/rtmp/fuzz/fuzz_targets/fuzz_*.rs; do
    target="$(basename "$f" .rs)"
    TARGETS+=("$target")
  done
  shopt -u nullglob
fi

echo "[fuzz-smoke] runtime fuzz smoke enabled"
echo "[fuzz-smoke] targets: ${TARGETS[*]}"
echo "[fuzz-smoke] max_total_time per target: ${MAX_TOTAL_TIME}s"

if ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "[fuzz-smoke] cargo-fuzz is required in PATH when RUN_FUZZ_SMOKE_RUN=1"
  echo "[fuzz-smoke] install with: cargo +nightly install cargo-fuzz --locked"
  exit 1
fi

for target in "${TARGETS[@]}"; do
  echo "[fuzz-smoke] running ${target}"
  cargo +nightly fuzz run --fuzz-dir crates/protocols/rtmp/fuzz "${target}" -- -max_total_time="${MAX_TOTAL_TIME}"
done

echo "[fuzz-smoke] runtime fuzz smoke completed"
