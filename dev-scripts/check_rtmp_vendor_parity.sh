#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

VENDOR_ROOT="vendor-ref/rtmp-rs"
LOCAL_PROPERTY_TESTS_ROOT="crates/cheetah-rtmp-property-tests/tests"
LOCAL_FUZZ_ROOT="crates/cheetah-rtmp-fuzz/fuzz_targets"
LOCAL_CAPI_SRC="crates/cheetah-rtmp-c-api/src"
LOCAL_WASM_SRC="crates/cheetah-rtmp-wasm/src"

echo "[parity] checking vendor parity against local RTMP suites"

require_dir() {
  local dir="$1"
  if [[ ! -d "${dir}" ]]; then
    echo "[parity] missing directory: ${dir}"
    exit 1
  fi
}

require_dir "${VENDOR_ROOT}/tests"
require_dir "${VENDOR_ROOT}/pbt/tests"
require_dir "${VENDOR_ROOT}/fuzz/fuzz_targets"
require_dir "${LOCAL_PROPERTY_TESTS_ROOT}"
require_dir "${LOCAL_FUZZ_ROOT}"
require_dir "${LOCAL_CAPI_SRC}"
require_dir "${LOCAL_WASM_SRC}"

tmp_vendor="$(mktemp)"
tmp_local="$(mktemp)"
vendor_tests="$(mktemp)"
local_tests="$(mktemp)"
trap 'rm -f "${tmp_vendor}" "${tmp_local}" "${vendor_tests}" "${local_tests}"' EXIT

assert_vendor_subset_in_local() {
  local label="$1"
  local vendor_dir="$2"
  local local_dir="$3"
  local glob="$4"

  find "${vendor_dir}" -maxdepth 1 -type f -name "${glob}" -printf "%f\n" | sort -u > "${tmp_vendor}"
  find "${local_dir}" -maxdepth 1 -type f -name "${glob}" -printf "%f\n" | sort -u > "${tmp_local}"

  local missing
  missing="$(comm -23 "${tmp_vendor}" "${tmp_local}" || true)"
  if [[ -n "${missing}" ]]; then
    echo "[parity] ${label} missing in local:"
    echo "${missing}"
    exit 1
  fi

  echo "[parity] ${label}: ok"
}

assert_vendor_subset_in_local "integration tests (*.rs)" \
  "${VENDOR_ROOT}/tests" \
  "${LOCAL_PROPERTY_TESTS_ROOT}" \
  "*.rs"

assert_vendor_subset_in_local "property tests (*.rs)" \
  "${VENDOR_ROOT}/pbt/tests" \
  "${LOCAL_PROPERTY_TESTS_ROOT}" \
  "*.rs"

assert_vendor_subset_in_local "fuzz targets (*.rs)" \
  "${VENDOR_ROOT}/fuzz/fuzz_targets" \
  "${LOCAL_FUZZ_ROOT}" \
  "*.rs"

assert_vendor_subset_in_local "AMF fixtures (*.bin)" \
  "${VENDOR_ROOT}/tests/testdata" \
  "${LOCAL_PROPERTY_TESTS_ROOT}/testdata" \
  "*.bin"

echo "[parity] checking property-tests/fuzz use main cheetah-rtmp-core"
if ! rg -q 'cheetah-rtmp-core\s*=\s*\{[^}]*path\s*=\s*"\.\./cheetah-rtmp-core"' \
  crates/cheetah-rtmp-property-tests/Cargo.toml crates/cheetah-rtmp-fuzz/Cargo.toml; then
  echo "[parity] expected cheetah-rtmp-property-tests and cheetah-rtmp-fuzz to depend on ../cheetah-rtmp-core"
  exit 1
fi

echo "[parity] checking executable test parity (vendor subset of local)"
cargo test --manifest-path "${VENDOR_ROOT}/pbt/Cargo.toml" -- --list \
  | awk -F': test' '/: test$/{print $1}' \
  | sort -u > "${tmp_vendor}"

cargo test --manifest-path "${VENDOR_ROOT}/Cargo.toml" \
  --test rtmp_play_client_test --test rtmp_publish_client_test -- --list \
  | awk -F': test' '/: test$/{print $1}' \
  | sort -u > "${tmp_local}"

cat "${tmp_vendor}" "${tmp_local}" | sort -u > "${vendor_tests}"

cargo test -p cheetah-rtmp-property-tests -- --list \
  | awk -F': test' '/: test$/{print $1}' \
  | sort -u > "${local_tests}"

missing_vendor_tests="$(comm -23 "${vendor_tests}" "${local_tests}" || true)"
if [[ -n "${missing_vendor_tests}" ]]; then
  echo "[parity] missing executable tests in local suite:"
  echo "${missing_vendor_tests}"
  exit 1
fi
echo "[parity] executable test parity: ok"

echo "[parity] checking no compatibility-layer symbols in RTMP src"
if rg -n "pbt_impl|legacy_compat|compat_layer|vendor_compat|shim_" \
  crates/cheetah-rtmp-core/src \
  crates/cheetah-rtmp-driver-tokio/src \
  crates/cheetah-rtmp-module/src \
  "${LOCAL_CAPI_SRC}" \
  "${LOCAL_WASM_SRC}" >/dev/null 2>&1; then
  echo "[parity] found forbidden compatibility-layer symbols"
  rg -n "pbt_impl|legacy_compat|compat_layer|vendor_compat|shim_" \
    crates/cheetah-rtmp-core/src \
    crates/cheetah-rtmp-driver-tokio/src \
    crates/cheetah-rtmp-module/src \
    "${LOCAL_CAPI_SRC}" \
    "${LOCAL_WASM_SRC}"
  exit 1
fi

echo "[parity] all vendor parity checks passed"
