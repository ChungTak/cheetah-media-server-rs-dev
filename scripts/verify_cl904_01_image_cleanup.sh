#!/usr/bin/env bash
# CL904-01: verify that no third-party `image` crate dependency remains.
# The media server owns its `image` module in `cheetah-codec` and `cheetah-media-api`;
# the third-party `image` crate must not appear in Cargo.toml, Cargo.lock or source imports.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail() {
    printf 'FAIL: %b\n' "$1" >&2
    exit 1
}

# 1. Cargo.lock must not contain a package named `image`.
if grep -Eq '^name = "image"$' Cargo.lock; then
    fail "third-party 'image' crate found in Cargo.lock"
fi

# 2. Cargo.toml files must not declare a direct `image` dependency.
mapfile -t files < <(find . -name Cargo.toml -not -path './target/*')
if grep -RsnE '^image[[:space:]]*=|^image\.workspace' "${files[@]}" 2>/dev/null; then
    fail "direct 'image' dependency found in a Cargo.toml"
fi

# 3. cargo tree must not resolve `image` for the server package.
cargo_tree_output=$(cargo tree -i image -p cheetah-server 2>&1 || true)
if ! grep -q "did not match any packages" <<<"$cargo_tree_output"; then
    fail "'image' crate still resolves in the dependency tree:\n$cargo_tree_output"
fi

# 4. Source imports: allow `pub mod image;` and `pub use image::{...}` (our own module),
#    but reject a bare `use image::` or `extern crate image` that would reference a crate.
if grep -RsnE '^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+image(::|;)' crates apps 2>/dev/null \
    | grep -vE 'pub[[:space:]]+use[[:space:]]+image::' > /tmp/cl904_01_image_imports.txt; then
    fail "direct third-party 'image' crate import found:\n$(cat /tmp/cl904_01_image_imports.txt)"
fi

if grep -RsnE '^[[:space:]]*extern[[:space:]]+crate[[:space:]]+image([[:space:];]|$)' crates apps 2>/dev/null > /tmp/cl904_01_image_extern.txt; then
    fail "direct third-party 'image' crate extern declaration found:\n$(cat /tmp/cl904_01_image_extern.txt)"
fi

echo "CL904-01 image cleanup: PASS"
