use crate::frame::AVFrame;
use crate::time::Timebase;
use crate::track::{CodecId, MediaKind, TrackInfo};
use crate::{TimestampNormalizeMode, TimestampValue};

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

fn is_video_frame(frame: &AVFrame) -> bool {
    frame.media_kind == MediaKind::Video && is_video_codec(frame.codec)
}

pub fn source_timeline_mode_for_rtp_ingress(frame: &AVFrame) -> TimestampNormalizeMode {
    if is_video_frame(frame) {
        TimestampNormalizeMode::DtsPts {
            dts: TimestampValue::Wrapped(i64_to_wrapped_u64(frame.pts)),
            pts: TimestampValue::Wrapped(i64_to_wrapped_u64(frame.pts)),
        }
    } else {
        TimestampNormalizeMode::DtsPts {
            dts: TimestampValue::Wrapped(i64_to_wrapped_u64(frame.dts)),
            pts: TimestampValue::Wrapped(i64_to_wrapped_u64(frame.pts)),
        }
    }
}

/// Convert an i64 timestamp (which may be negative due to B-frame composition
/// offsets) to a u64 suitable for `TimestampValue::Wrapped`. Negative values
/// are clamped to 0 since the wrapped RTP timestamp space is unsigned.
fn i64_to_wrapped_u64(ts: i64) -> u64 {
    ts.max(0) as u64
}

pub fn monotonic_dts_min_step(timebase: Timebase) -> i64 {
    let num = u64::from(timebase.num.max(1));
    let den = u64::from(timebase.den.max(1));
    let denom = num.saturating_mul(1_000);
    let step = den.saturating_add(denom.saturating_sub(1)) / denom;
    i64::try_from(step.max(1)).unwrap_or(i64::MAX)
}

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
    fn source_mode_uses_pts_for_video_dts_pts_and_audio_keeps_dts() {
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
                assert_eq!(dts, TimestampValue::Wrapped(1024));
                assert_eq!(pts, TimestampValue::Wrapped(2048));
            }
            _ => panic!("unexpected mode"),
        }
    }
}
