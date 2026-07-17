#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

CRATE="cheetah-rtmp-core"
TARGETS=(
  "wasm32-unknown-unknown"
  "thumbv7em-none-eabihf"
)

echo "[no-std] checking ${CRATE} on host target"
cargo check -p "${CRATE}"
echo "[no-std] running ${CRATE} no-std smoke tests on host target"
cargo test -p "${CRATE}" --lib

installed_targets="$(rustup target list --installed)"

for target in "${TARGETS[@]}"; do
  if grep -qx "${target}" <<<"${installed_targets}"; then
    echo "[no-std] checking ${CRATE} on ${target}"
    cargo check -p "${CRATE}" --target "${target}"
    continue
  fi

  if [[ "${STRICT_TARGETS:-0}" == "1" ]]; then
    echo "[no-std] installing missing target: ${target}"
    rustup target add "${target}"
    echo "[no-std] checking ${CRATE} on ${target}"
    cargo check -p "${CRATE}" --target "${target}"
    continue
  fi

  echo "[no-std] skip ${target} (not installed; set STRICT_TARGETS=1 to fail)"
done
