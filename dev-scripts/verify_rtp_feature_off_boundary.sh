#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

echo "[rtp-feature-off] checking cheetah-server --no-default-features does not pull cheetah-rtp-module"
output=$(cargo tree -p cheetah-server --no-default-features -i cheetah-rtp-module --offline 2>&1 || true)
if ! grep -q "did not match any packages" <<<"$output"; then
  echo "[rtp-feature-off] cheetah-rtp-module should not be in the default server tree" >&2
  echo "$output" >&2
  exit 1
fi

echo "[rtp-feature-off] checking cheetah-server --features rtp includes cheetah-rtp-module"
output=$(cargo tree -p cheetah-server --no-default-features --features rtp -i cheetah-rtp-module --offline 2>&1 || true)
if ! grep -q "cheetah-rtp-module" <<<"$output"; then
  echo "[rtp-feature-off] cheetah-rtp-module should resolve when the rtp feature is enabled" >&2
  echo "$output" >&2
  exit 1
fi

echo "[rtp-feature-off] ok"
