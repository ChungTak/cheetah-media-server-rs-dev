//! RtmpTimestamp / RtmpTimestampDelta 的 Property-Based Testing

use cheetah_rtmp_core::{RtmpTimestamp, RtmpTimestampDelta};
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// 验证 from_millis / as_millis 是可逆的
    #[test]
    fn timestamp_millis_roundtrip(ms in any::<u32>()) {
        let ts = RtmpTimestamp::from_millis(ms);
        prop_assert_eq!(ts.as_millis(), ms);
    }

    /// 验证 as_duration 返回正确的 Duration
    #[test]
    fn timestamp_as_duration(ms in any::<u32>()) {
        let ts = RtmpTimestamp::from_millis(ms);
        let expected = Duration::from_millis(ms as u64);
        prop_assert_eq!(ts.as_duration(), expected);
    }

    /// 验证 wrapping_add 在溢出时环绕
    #[test]
    fn timestamp_wrapping_add(a in any::<u32>(), b in any::<u32>()) {
        let ts_a = RtmpTimestamp::from_millis(a);
        let ts_b = RtmpTimestamp::from_millis(b);
        let result = ts_a.wrapping_add(ts_b);
        prop_assert_eq!(result.as_millis(), a.wrapping_add(b));
    }

    /// 验证 checked_sub 正确工作
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

    /// 验证 ZERO 常量为 0 毫秒
    #[test]
    fn timestamp_zero_is_zero(_dummy in Just(())) {
        prop_assert_eq!(RtmpTimestamp::ZERO.as_millis(), 0);
    }

    /// 验证 RtmpTimestampDelta 的 from_millis / as_millis 是可逆的
    #[test]
    fn timestamp_delta_millis_roundtrip(ms in any::<i32>()) {
        let delta = RtmpTimestampDelta::from_millis(ms);
        prop_assert_eq!(delta.as_millis(), ms);
    }

    /// 验证 RtmpTimestampDelta 的 ZERO 常量为 0 毫秒
    #[test]
    fn timestamp_delta_zero_is_zero(_dummy in Just(())) {
        prop_assert_eq!(RtmpTimestampDelta::ZERO.as_millis(), 0);
    }
}
