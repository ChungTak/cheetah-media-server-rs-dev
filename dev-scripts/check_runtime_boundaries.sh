#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

# Public-API crates whose exported types must stay runtime-neutral.
PUB_API_CRATES=(
  "crates/runtime/cheetah-runtime-api/src"
  "crates/sdk/cheetah-sdk/src"
  "crates/system/cheetah-engine/src"
  "crates/protocols/rtmp/module/src"
)

DRIVER_CRATE="crates/protocols/rtmp/driver-tokio/src"
MODULE_CRATE="crates/protocols/rtmp/module/src"

# Fail loudly if a checked path disappears (e.g. after a crate move) instead of
# silently scanning nothing and reporting success.
for path in "${PUB_API_CRATES[@]}" "${DRIVER_CRATE}" "${MODULE_CRATE}"; do
  if [[ ! -d "${path}" ]]; then
    echo "[boundary] expected source dir missing: ${path}" >&2
    echo "[boundary] update dev-scripts/check_runtime_boundaries.sh after crate moves" >&2
    exit 1
  fi
done

# Patterns use the default (Rust regex) engine so this guard does not depend on
# a PCRE2-enabled ripgrep build. A leading `(^|[^A-Za-z0-9_])` stands in for the
# negative-lookbehind word boundary so identifiers like `my_tokio::` do not match.
pub_tokio_pattern='^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]].*(^|[^A-Za-z0-9_])(tokio|tokio_util)::'
module_forbidden_pattern='(^|[^A-Za-z0-9_])tokio::(net|time|sync)::|(^|[^A-Za-z0-9_])tokio_util::sync::|(^|[^A-Za-z0-9_])tokio::select!'

echo "[boundary] checking public APIs do not expose tokio/tokio_util types"
if rg -n "${pub_tokio_pattern}" "${PUB_API_CRATES[@]}"; then
  echo "[boundary] found tokio/tokio_util type leakage in public API" >&2
  exit 1
fi

echo "[boundary] checking rtmp-module does not depend on forbidden tokio primitives"
if rg -n "${module_forbidden_pattern}" "${MODULE_CRATE}"; then
  echo "[boundary] found forbidden tokio usage in rtmp-module" >&2
  exit 1
fi

echo "[boundary] checking rtmp-driver-tokio public API stays runtime-neutral"
if rg -n "${pub_tokio_pattern}" "${DRIVER_CRATE}"; then
  echo "[boundary] found tokio/tokio_util type leakage in driver public API" >&2
  exit 1
fi

echo "[boundary] all runtime boundary checks passed"
