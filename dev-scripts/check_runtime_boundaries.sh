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
  "crates/protocols/webrtc/module/src"
  "crates/protocols/mp4/module/src"
)

DRIVER_CRATES=(
  "crates/protocols/rtmp/driver-tokio/src"
  "crates/protocols/webrtc/driver-tokio/src"
  "crates/protocols/mp4/driver-tokio/src"
)

# Module source dirs whose production code must not touch forbidden tokio
# primitives directly. Inline `#[cfg(test)]` code is covered too, so these
# modules must keep tokio out of `[dependencies]` (see manifest check below).
MODULE_CRATES=(
  "crates/protocols/rtmp/module/src"
  "crates/protocols/mp4/module/src"
)

# Module manifests whose `[dependencies]` section must stay tokio-free. A module
# that keeps tokio only as a dev-dependency cannot compile production code
# against `tokio::*`, which is a stronger guarantee than a source grep (and
# avoids false positives from inline `#[cfg(test)]` modules).
TOKIO_FREE_MODULE_MANIFESTS=(
  "crates/protocols/webrtc/module/Cargo.toml"
  "crates/protocols/mp4/module/Cargo.toml"
)

# Fail loudly if a checked path disappears (e.g. after a crate move) instead of
# silently scanning nothing and reporting success.
for path in "${PUB_API_CRATES[@]}" "${DRIVER_CRATES[@]}" "${MODULE_CRATES[@]}"; do
  if [[ ! -d "${path}" ]]; then
    echo "[boundary] expected source dir missing: ${path}" >&2
    echo "[boundary] update dev-scripts/check_runtime_boundaries.sh after crate moves" >&2
    exit 1
  fi
done

for manifest in "${TOKIO_FREE_MODULE_MANIFESTS[@]}"; do
  if [[ ! -f "${manifest}" ]]; then
    echo "[boundary] expected manifest missing: ${manifest}" >&2
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

echo "[boundary] checking modules do not depend on forbidden tokio primitives"
if rg -n "${module_forbidden_pattern}" "${MODULE_CRATES[@]}"; then
  echo "[boundary] found forbidden tokio usage in a module crate" >&2
  exit 1
fi

echo "[boundary] checking *-driver-tokio public APIs stay runtime-neutral"
if rg -n "${pub_tokio_pattern}" "${DRIVER_CRATES[@]}"; then
  echo "[boundary] found tokio/tokio_util type leakage in driver public API" >&2
  exit 1
fi

# The `[dependencies]` section runs until the next top-level `[` table header.
# A module that only lists tokio under `[dev-dependencies]` cannot compile
# production code against tokio, so a manifest check is a complete guarantee.
forbidden_dep_pattern='^[[:space:]]*(tokio|tokio-util|tokio-rustls|tokio-tungstenite)[[:space:]]*(=|\.|\{)'
for manifest in "${TOKIO_FREE_MODULE_MANIFESTS[@]}"; do
  echo "[boundary] checking ${manifest} keeps tokio out of [dependencies]"
  deps_section="$(awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f' "${manifest}")"
  if printf '%s\n' "${deps_section}" | rg -n "${forbidden_dep_pattern}"; then
    echo "[boundary] found forbidden tokio dependency in ${manifest} [dependencies]" >&2
    echo "[boundary] move it to [dev-dependencies]; production code must stay runtime-neutral" >&2
    exit 1
  fi
done

echo "[boundary] all runtime boundary checks passed"
