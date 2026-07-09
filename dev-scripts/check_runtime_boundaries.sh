#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

PUB_API_CRATES=(
  "crates/cheetah-runtime-api/src"
  "crates/cheetah-sdk/src"
  "crates/cheetah-engine/src"
  "crates/cheetah-rtmp-module/src"
)

DRIVER_CRATE="crates/cheetah-rtmp-driver-tokio/src"
MODULE_CRATE="crates/cheetah-rtmp-module/src"

pub_tokio_pattern='^\s*pub(?:\s*\([^)]*\))?\s+.*(?<![A-Za-z0-9_])(tokio|tokio_util)::'
module_forbidden_pattern='(?<![A-Za-z0-9_])tokio::(net|time|sync)::|(?<![A-Za-z0-9_])tokio_util::sync::|(?<![A-Za-z0-9_])tokio::select!'

echo "[boundary] checking public APIs do not expose tokio/tokio_util types"
if rg --pcre2 -n "${pub_tokio_pattern}" "${PUB_API_CRATES[@]}"; then
  echo "[boundary] found tokio/tokio_util type leakage in public API" >&2
  exit 1
fi

echo "[boundary] checking rtmp-module does not depend on forbidden tokio primitives"
if rg --pcre2 -n "${module_forbidden_pattern}" "${MODULE_CRATE}"; then
  echo "[boundary] found forbidden tokio usage in rtmp-module" >&2
  exit 1
fi

echo "[boundary] checking rtmp-driver-tokio public API stays runtime-neutral"
if rg --pcre2 -n "${pub_tokio_pattern}" "${DRIVER_CRATE}"; then
  echo "[boundary] found tokio/tokio_util type leakage in driver public API" >&2
  exit 1
fi

echo "[boundary] all runtime boundary checks passed"
