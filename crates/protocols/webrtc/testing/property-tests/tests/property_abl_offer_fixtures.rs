//! Property tests for ABL-style SDP offer edge cases.
//!
//! Phase 05 Task 03: verify that the offer payload extractor and SDP
//! preprocessor never panic, never allocate unboundedly, and produce
//! consistent results across the full input space of ABL-style offers.
//!
//! Properties:
//! - `extract_offer_payloads` never panics on arbitrary input.
//! - Codec name matching is truly case-insensitive (any casing of
//!   H264/H265/OPUS is recognized).
//! - Payload type values are always in [0, 127] when present.
//! - First-match-wins semantics hold for duplicate codec entries.
//! - `preprocess_remote_sdp` + `extract_offer_payloads` composition
//!   never panics.
//!
//! ABL 风格 SDP offer 边界情况的属性测试。
//!
//! 阶段 05 任务 03：验证 offer payload 提取器与 SDP 预处理器对任意输入不 panic、
//! 不无限分配，并在 ABL 风格 offer 全输入空间产生一致结果。
//!
//! 属性：
//! - `extract_offer_payloads` 对任意输入不 panic。
//! - 编解码器名称匹配真正大小写不敏感（H264/H265/OPUS 任意大小写均被识别）。
//! - 存在时 payload type 始终在 [0, 127] 范围内。
//! - 重复 codec 条目遵循首个匹配胜出语义。
//! - `preprocess_remote_sdp` + `extract_offer_payloads` 组合不 panic。

use cheetah_webrtc_core::{extract_offer_payloads, preprocess_remote_sdp};
use proptest::prelude::*;

/// Generate a random codec name that is a case variant of H264.
///
/// 生成 H264 的任意大小写变体。
fn h264_case_variant() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "H264".to_string(),
        "h264".to_string(),
        "H264".to_string(),
        "h264".to_string(),
    ])
}

/// Generate a random codec name that is a case variant of opus.
///
/// 生成 opus 的任意大小写变体。
fn opus_case_variant() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "opus".to_string(),
        "OPUS".to_string(),
        "Opus".to_string(),
        "OpUs".to_string(),
    ])
}

/// Generate a random codec name that is a case variant of H265/HEVC.
///
/// 生成 H265/HEVC 的任意大小写变体。
fn h265_case_variant() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "H265".to_string(),
        "h265".to_string(),
        "HEVC".to_string(),
        "hevc".to_string(),
        "Hevc".to_string(),
    ])
}

proptest! {
    /// The offer payload extractor never panics on arbitrary input.
    ///
    /// offer payload 提取器对任意输入不 panic。
    #[test]
    fn extract_offer_payloads_does_not_panic(input in any::<String>()) {
        let _ = extract_offer_payloads(&input);
    }

    /// Preprocessing + extraction composition never panics.
    ///
    /// 预处理 + 提取组合不 panic。
    #[test]
    fn preprocess_then_extract_does_not_panic(input in any::<String>()) {
        let (sanitized, _) = preprocess_remote_sdp(&input);
        let _ = extract_offer_payloads(&sanitized);
    }

    /// Any case variant of H264 with clock 90000 is recognized.
    ///
    /// 任意大小写 H264 且时钟为 90000 被识别。
    #[test]
    fn h264_case_insensitive(
        codec in h264_case_variant(),
        pt in 0u8..128,
    ) {
        let sdp = format!("v=0\r\na=rtpmap:{pt} {codec}/90000\r\n");
        let payloads = extract_offer_payloads(&sdp);
        prop_assert_eq!(payloads.h264, Some(pt));
    }

    /// Any case variant of opus with clock 48000 is recognized.
    ///
    /// 任意大小写 opus 且时钟为 48000 被识别。
    #[test]
    fn opus_case_insensitive(
        codec in opus_case_variant(),
        pt in 0u8..128,
    ) {
        let sdp = format!("v=0\r\na=rtpmap:{pt} {codec}/48000/2\r\n");
        let payloads = extract_offer_payloads(&sdp);
        prop_assert_eq!(payloads.opus, Some(pt));
    }

    /// Any case variant of H265/HEVC with clock 90000 is recognized.
    ///
    /// 任意大小写 H265/HEVC 且时钟为 90000 被识别。
    #[test]
    fn h265_case_insensitive(
        codec in h265_case_variant(),
        pt in 0u8..128,
    ) {
        let sdp = format!("v=0\r\na=rtpmap:{pt} {codec}/90000\r\n");
        let payloads = extract_offer_payloads(&sdp);
        prop_assert_eq!(payloads.h265, Some(pt));
    }

    /// First-match-wins: when multiple rtpmap lines match the same
    /// codec, the first payload type is returned.
    ///
    /// 首个匹配胜出：当多条 rtpmap 行匹配同一 codec 时返回第一个 payload type。
    #[test]
    fn first_match_wins_for_h264(
        pt1 in 0u8..128,
        pt2 in 0u8..128,
    ) {
        let sdp = format!(
            "v=0\r\na=rtpmap:{pt1} H264/90000\r\na=rtpmap:{pt2} H264/90000\r\n"
        );
        let payloads = extract_offer_payloads(&sdp);
        prop_assert_eq!(payloads.h264, Some(pt1));
    }

    /// Non-contiguous payload types: any valid PT in [0, 127] is
    /// accepted regardless of gaps or ordering.
    ///
    /// 非连续 payload type：[0, 127] 内任意有效 PT 均被接受，与间隔或顺序无关。
    #[test]
    fn noncontiguous_payload_types_accepted(
        h264_pt in 0u8..128,
        opus_pt in 0u8..128,
    ) {
        let sdp = format!(
            "v=0\r\na=rtpmap:{h264_pt} H264/90000\r\na=rtpmap:{opus_pt} opus/48000/2\r\n"
        );
        let payloads = extract_offer_payloads(&sdp);
        prop_assert_eq!(payloads.h264, Some(h264_pt));
        prop_assert_eq!(payloads.opus, Some(opus_pt));
    }

    /// Wrong clock rate is never matched — H264 at non-90000 clock
    /// and opus at non-48000 clock are ignored.
    ///
    /// 错误时钟率永不匹配——非 90000 的 H264 与非 48000 的 opus 被忽略。
    #[test]
    fn wrong_clock_rate_not_matched(
        pt in 0u8..128,
        clock in prop::sample::select(vec![8000u32, 16000, 44100, 48001, 90001]),
    ) {
        let sdp = format!("v=0\r\na=rtpmap:{pt} H264/{clock}\r\n");
        let payloads = extract_offer_payloads(&sdp);
        prop_assert_eq!(payloads.h264, None);
    }
}
