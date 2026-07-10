use crate::frame::AVFrame;
use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackInfo};
use crate::{TimestampNormalizeMode, TimestampValue};

/// Returns true for codecs that produce video frames.
///
/// 返回是否为产生视频帧的编解码器。
fn is_video_codec(codec: CodecId) -> bool {
    matches!(
        codec,
        CodecId::H264
            | CodecId::H265
            | CodecId::H266
            | CodecId::AV1
            | CodecId::VP8
            | CodecId::VP9
            | CodecId::MJPEG
    )
}

/// Returns true if the frame is video from a video codec.
///
/// 返回该帧是否为来自视频编解码器的视频帧。
fn is_video_frame(frame: &AVFrame) -> bool {
    frame.media_kind == MediaKind::Video && is_video_codec(frame.codec)
}

/// Choose the timestamp normalization mode for an RTP-ingress frame.
///
/// RTP ingress carries a single wall-clock timestamp. The normalizer receives it as
/// both DTS and PTS so it can build a monotonic decode timeline while keeping the
/// original value for composition offsets.
///
/// 为 RTP 入口帧选择时间戳归一化模式。
///
/// RTP 入口只携带一个墙上时间戳。归一化器同时将其作为 DTS 和 PTS 接收，
/// 从而在保留原始值用于合成偏移的同时构建单调解码时间线。
pub fn source_timeline_mode_for_rtp_ingress(frame: &AVFrame) -> TimestampNormalizeMode {
    // RTP ingress always provides a single wall-clock timestamp. For audio it
    // is supplied as both DTS and PTS; for video the same presentation value is
    // used as both the source decode and presentation candidate so the
    // normalizer can clamp to a monotonic DTS while preserving the original PTS
    // for composition offsets.
    TimestampNormalizeMode::DtsPts {
        dts: TimestampValue::Wrapped(i64_to_wrapped_u64(frame.pts)),
        pts: TimestampValue::Wrapped(i64_to_wrapped_u64(frame.pts)),
    }
}

/// Convert a possibly-negative i64 timestamp to a wrapped u64 timestamp.
///
/// Negative values are clamped to 0 because the wrapped RTP timestamp space is unsigned.
///
/// 将可能为负的 i64 时间戳转换为包裹的 u64 时间戳。
///
/// 负值被限制为 0，因为包裹的 RTP 时间戳空间无符号。
fn i64_to_wrapped_u64(ts: i64) -> u64 {
    ts.max(0) as u64
}

/// Compute the smallest DTS step that still advances by at least one millisecond.
///
/// Used to prevent pathological non-monotonicity when the timebase has very high
/// denominator (e.g. 90kHz) and callers try to use a zero or tiny fallback step.
///
/// 计算仍使 DTS 至少前进 1 毫秒的最小步长。
///
/// 用于防止 timebase 分母很大（如 90kHz）且调用方使用零或微小回退步长时
/// 出现病态非单调。
pub fn monotonic_dts_min_step(timebase: Timebase) -> i64 {
    let num = u64::from(timebase.num.max(1));
    let den = u64::from(timebase.den.max(1));
    let denom = num.saturating_mul(1_000);
    let step = den.saturating_add(denom.saturating_sub(1)) / denom;
    i64::try_from(step.max(1)).unwrap_or(i64::MAX)
}

/// Determine the fallback DTS step for an RTP-ingress frame.
///
/// Prefers the explicit frame duration. For video, it falls back to the frame rate
/// derived from `TrackInfo.fps`, then to `clock_rate / 30`. For audio, it uses the
/// minimum step that keeps DTS monotonic.
///
/// 确定 RTP 入口帧的回退 DTS 步长。
///
/// 优先使用显式帧时长。视频则回退到 `TrackInfo.fps` 推导的帧率，再回退到
/// `clock_rate / 30`。音频使用保持 DTS 单调的最小步长。
pub fn fallback_step_for_rtp_ingress(
    track: &TrackInfo,
    frame: &AVFrame,
    timebase: Timebase,
) -> i64 {
    let min_step = monotonic_dts_min_step(timebase);
    if frame.duration > 0 {
        return frame.duration.max(min_step);
    }
    if is_video_frame(frame) {
        if let Some(fps) = track.fps {
            if fps.num > 0 && fps.den > 0 {
                let den = i128::from(timebase.den.max(1));
                let numerator = den.saturating_mul(i128::from(fps.den));
                let step = numerator / i128::from(fps.num);
                if let Ok(step_i64) = i64::try_from(step.max(i128::from(1))) {
                    return step_i64.max(min_step);
                }
            }
        }
        let by_clock = i64::from((track.clock_rate / 30).max(1));
        return by_clock.max(min_step);
    }
    min_step
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameFormat, TrackId};
    use bytes::Bytes;

    #[test]
    fn source_mode_uses_dts_pts_with_presentation_as_dts_for_video() {
        let video = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            900,
            450,
            Timebase::new(1, 90_000),
            Bytes::from_static(b"v"),
        );
        let audio = AVFrame::new(
            TrackId(2),
            MediaKind::Audio,
            CodecId::AAC,
            FrameFormat::AacRaw,
            2048,
            1024,
            Timebase::new(1, 48_000),
            Bytes::from_static(b"a"),
        );

        match source_timeline_mode_for_rtp_ingress(&video) {
            TimestampNormalizeMode::DtsPts { dts, pts } => {
                assert_eq!(dts, TimestampValue::Wrapped(900));
                assert_eq!(pts, TimestampValue::Wrapped(900));
            }
            _ => panic!("unexpected mode"),
        }

        match source_timeline_mode_for_rtp_ingress(&audio) {
            TimestampNormalizeMode::DtsPts { dts, pts } => {
                assert_eq!(dts, TimestampValue::Wrapped(2048));
                assert_eq!(pts, TimestampValue::Wrapped(2048));
            }
            _ => panic!("unexpected mode"),
        }
    }
}
