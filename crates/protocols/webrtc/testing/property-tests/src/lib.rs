//! Property-test scaffold for `cheetah-webrtc-core`.
//!
//! Tests live under `tests/` and use [`proptest`]. The crate itself is
//! a marker so the workspace member resolves; later phases extend the
//! property suite with timer/network/SDP fuzzing.

pub fn webrtc_property_tests_marker() -> &'static str {
    "cheetah-webrtc-property-tests"
}
