//! Property-based tests for `RtmpTimestamp` and `RtmpTimestampDelta`.
//!
//! These tests verify the basic arithmetic of the timestamp newtypes: construction,
//! duration conversion, wrapping addition, and checked subtraction. The RTMP wire
//! format uses 32-bit millisecond timestamps, so wrapping arithmetic is a core invariant.
//!
//! `RtmpTimestamp` 与 `RtmpTimestampDelta` 的属性测试。
//!
//! 这些测试校验时间戳 newtype 的基本运算：构造、Duration 转换、环绕加法以及 checked 减法。
//! RTMP 线格式使用 32 位毫秒时间戳，因此环绕算术是核心不变量。

use cheetah_rtmp_core::{RtmpTimestamp, RtmpTimestampDelta};
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Verify that `from_millis` and `as_millis` are inverses for any u32 value.
    ///
    /// 校验对任意 u32 值，`from_millis` 与 `as_millis` 互为逆运算。
    #[test]
    fn timestamp_millis_roundtrip(ms in any::<u32>()) {
        let ts = RtmpTimestamp::from_millis(ms);
        prop_assert_eq!(ts.as_millis(), ms);
    }

    /// Verify that `as_duration` returns the expected `Duration`.
    ///
    /// 校验 `as_duration` 返回正确的 `Duration`。
    #[test]
    fn timestamp_as_duration(ms in any::<u32>()) {
        let ts = RtmpTimestamp::from_millis(ms);
        let expected = Duration::from_millis(ms as u64);
        prop_assert_eq!(ts.as_duration(), expected);
    }

    /// Verify that `wrapping_add` wraps on 32-bit overflow.
    ///
    /// 校验 `wrapping_add` 在 32 位溢出时环绕。
    #[test]
    fn timestamp_wrapping_add(a in any::<u32>(), b in any::<u32>()) {
        let ts_a = RtmpTimestamp::from_millis(a);
        let ts_b = RtmpTimestamp::from_millis(b);
        let result = ts_a.wrapping_add(ts_b);
        prop_assert_eq!(result.as_millis(), a.wrapping_add(b));
    }

    /// Verify that `checked_sub` returns `Some` only when the subtraction is non-negative.
    ///
    /// 校验 `checked_sub` 仅在减法结果非负时返回 `Some`。
    #[test]
    fn timestamp_checked_sub(a in any::<u32>(), b in any::<u32>()) {
        let ts_a = RtmpTimestamp::from_millis(a);
        let ts_b = RtmpTimestamp::from_millis(b);
        let result = ts_a.checked_sub(ts_b);
        if a >= b {
            prop_assert!(result.is_some());
            prop_assert_eq!(result.unwrap().as_millis(), a - b);
        } else {
            prop_assert!(result.is_none());
        }
    }

    /// Verify that `RtmpTimestamp::ZERO` is exactly 0 ms.
    ///
    /// 校验 `RtmpTimestamp::ZERO` 等于 0 毫秒。
    #[test]
    fn timestamp_zero_is_zero(_dummy in Just(())) {
        prop_assert_eq!(RtmpTimestamp::ZERO.as_millis(), 0);
    }

    /// Verify that `RtmpTimestampDelta::from_millis` and `as_millis` are inverses.
    ///
    /// 校验 `RtmpTimestampDelta::from_millis` 与 `as_millis` 互为逆运算。
    #[test]
    fn timestamp_delta_millis_roundtrip(ms in any::<i32>()) {
        let delta = RtmpTimestampDelta::from_millis(ms);
        prop_assert_eq!(delta.as_millis(), ms);
    }

    /// Verify that `RtmpTimestampDelta::ZERO` is exactly 0 ms.
    ///
    /// 校验 `RtmpTimestampDelta::ZERO` 等于 0 毫秒。
    #[test]
    fn timestamp_delta_zero_is_zero(_dummy in Just(())) {
        prop_assert_eq!(RtmpTimestampDelta::ZERO.as_millis(), 0);
    }
}
