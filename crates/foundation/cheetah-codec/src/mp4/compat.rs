//! MP4 compatibility/quirk helpers.
//!
//! MP4 兼容性与怪癖辅助函数。
//!
//! Encapsulates non-standard inputs the reader/writer must tolerate without
//! panicking or unbounded allocation. Each helper is a small, named function
//! so cross-protocol compat decisions stay traceable.
/// Returns true for box 4cc that should be treated as transparent skip-able
/// padding inside `moov`/`trak`/`mdia` or top-level streams.
///
/// 若该 4cc 应作为 `moov`/`trak`/`mdia` 或顶层流中的透明可跳过填充，则返回 true。
pub fn is_skippable_top_level_box(fourcc: &[u8; 4]) -> bool {
    matches!(
        fourcc,
        b"free" | b"skip" | b"wide" | b"uuid" | b"meta" | b"sbgp" | b"sgpd"
    )
}

/// Returns true if the supplied 4cc is one of the known sample entry codings
/// that the reader can handle. Inputs outside this list still pass through
/// the read pipeline as unknown, and the reader must downgrade gracefully.
///
/// 若提供的 4cc 是读取器可处理的已知样本条目编码则返回 true。
/// 列表之外的输入仍作为未知通过读取管线，读取器需优雅降级。
pub fn is_supported_sample_entry(fourcc: &[u8; 4]) -> bool {
    matches!(
        fourcc,
        b"avc1"
            | b"avc2"
            | b"avc3"
            | b"avc4"
            | b"hvc1"
            | b"hev1"
            | b"dvh1"
            | b"dvhe"
            | b"vvc1"
            | b"vp08"
            | b"vp09"
            | b"av01"
            | b"mp4v"
            | b"jpeg"
            | b"mjpa"
            | b"mjpb"
            | b"mp4a"
            | b"alaw"
            | b"ulaw"
            | b"Opus"
            | b"opus"
    )
}

/// Clamp `ctts` composition offsets to a sane range. Some encoders emit
/// extreme values that overflow i32 arithmetic when converted between
/// timescales.
///
/// 将 `ctts` 合成偏移限制在合理范围。某些编码器在 timebase 转换时
/// 会输出导致 i32 运算溢出的极端值。
pub fn clamp_composition_offset(value: i64) -> i32 {
    value.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skippable_top_level_set() {
        assert!(is_skippable_top_level_box(b"free"));
        assert!(is_skippable_top_level_box(b"skip"));
        assert!(is_skippable_top_level_box(b"wide"));
        assert!(is_skippable_top_level_box(b"uuid"));
        assert!(!is_skippable_top_level_box(b"moov"));
        assert!(!is_skippable_top_level_box(b"mdat"));
    }

    #[test]
    fn supported_sample_entries_cover_codec_matrix() {
        assert!(is_supported_sample_entry(b"avc1"));
        assert!(is_supported_sample_entry(b"hvc1"));
        assert!(is_supported_sample_entry(b"av01"));
        assert!(is_supported_sample_entry(b"mp4a"));
        assert!(is_supported_sample_entry(b"Opus"));
    }

    #[test]
    fn clamp_composition_offset_truncates() {
        assert_eq!(clamp_composition_offset(0), 0);
        assert_eq!(clamp_composition_offset(i64::MAX), i32::MAX);
        assert_eq!(clamp_composition_offset(i64::MIN), i32::MIN);
    }
}
