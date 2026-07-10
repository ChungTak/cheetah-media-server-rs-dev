//! Property-test scaffold for `cheetah-webrtc-core`.
//!
//! Tests live under `tests/` and use [`proptest`]. The crate itself is a
//! marker so the workspace member resolves; later phases extend the property
//! suite with timer/network/SDP fuzzing.
//!
//! `cheetah-webrtc-core` 的属性测试脚手架。
//!
//! 测试位于 `tests/` 并使用 [`proptest`]。本 crate 本身是一个标记，用于解析
//! workspace 成员；后续阶段将扩展 timer/network/SDP fuzzing 属性套件。

/// Marker function used to keep the test crate identifiable when loaded.
///
/// 标记函数，用于在加载时保持测试 crate 可识别。
pub fn webrtc_property_tests_marker() -> &'static str {
    "cheetah-webrtc-property-tests"
}
