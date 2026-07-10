//! Property-based tests for the `MediaFrame` enum and its audio/video variants.
//!
//! These tests focus on the value-level invariants of the RTMP media types:
//! variant constructors, cloning, and the raw-id ranges of the underlying enums.
//!
//! `MediaFrame` 枚举及其音频/视频变体的属性测试。
//!
//! 这些测试关注 RTMP 媒体类型的值级不变量：变体构造器、Clone 以及底层枚举原始 id 的范围。

use cheetah_rtmp_core::{
    AudioFormat, AudioFrame, AudioSampleRate, AvcPacketType, MediaFrame, RtmpTimestamp,
    RtmpTimestampDelta, VideoCodec, VideoFrame, VideoFrameType,
};
use proptest::prelude::*;

/// Generate an `AudioFormat` covering every FLV sound format id.
///
/// 生成覆盖所有 FLV 声音格式 id 的 `AudioFormat`。
fn arb_audio_format() -> impl Strategy<Value = AudioFormat> {
    prop_oneof![
        Just(AudioFormat::Adpcm),
        Just(AudioFormat::Mp3),
        Just(AudioFormat::LinearPcmLittleEndian),
        Just(AudioFormat::Nellymoser16khzMono),
        Just(AudioFormat::Nellymoser8KhzMono),
        Just(AudioFormat::Nellymoser),
        Just(AudioFormat::G711AlawLogarithmicPcm),
        Just(AudioFormat::G711MuLawLogarithmicPcm),
        Just(AudioFormat::Aac),
        Just(AudioFormat::Speex),
        Just(AudioFormat::Mp3_8khz),
        Just(AudioFormat::DeviceSpecificSound),
    ]
}

/// Generate an `AudioSampleRate`.
///
/// 生成 `AudioSampleRate`。
fn arb_audio_sample_rate() -> impl Strategy<Value = AudioSampleRate> {
    prop_oneof![
        Just(AudioSampleRate::Khz5),
        Just(AudioSampleRate::Khz11),
        Just(AudioSampleRate::Khz22),
        Just(AudioSampleRate::Khz44),
    ]
}

/// Generate an `AudioFrame`.
///
/// 生成 `AudioFrame`。
fn arb_audio_frame() -> impl Strategy<Value = AudioFrame> {
    (
        any::<u32>(),
        arb_audio_format(),
        arb_audio_sample_rate(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop::collection::vec(any::<u8>(), 0..64),
    )
        .prop_map(
            |(
                timestamp,
                format,
                sample_rate,
                is_8bit_sample,
                is_stereo,
                is_aac_sequence_header,
                data,
            )| AudioFrame {
                timestamp: RtmpTimestamp::from_millis(timestamp),
                format,
                sample_rate,
                is_8bit_sample,
                is_stereo,
                is_aac_sequence_header,
                data,
            },
        )
}

/// Generate a `VideoFrameType`.
///
/// 生成 `VideoFrameType`。
fn arb_video_frame_type() -> impl Strategy<Value = VideoFrameType> {
    prop_oneof![
        Just(VideoFrameType::KeyFrame),
        Just(VideoFrameType::InterFrame),
        Just(VideoFrameType::DisposableInterFrame),
        Just(VideoFrameType::GeneratedKeyFrame),
        Just(VideoFrameType::VideoInfoOrCommandFrame),
    ]
}

/// Generate a `VideoCodec`.
///
/// 生成 `VideoCodec`。
fn arb_video_codec() -> impl Strategy<Value = VideoCodec> {
    prop_oneof![
        Just(VideoCodec::Jpeg),
        Just(VideoCodec::H263),
        Just(VideoCodec::ScreenVideo),
        Just(VideoCodec::Vp6),
        Just(VideoCodec::Vp6WithAlpha),
        Just(VideoCodec::ScreenVideoV2),
        Just(VideoCodec::Avc),
    ]
}

/// Generate an `AvcPacketType`.
///
/// 生成 `AvcPacketType`。
fn arb_avc_packet_type() -> impl Strategy<Value = AvcPacketType> {
    prop_oneof![
        Just(AvcPacketType::SequenceHeader),
        Just(AvcPacketType::NalUnit),
        Just(AvcPacketType::EndOfSequence),
    ]
}

/// Generate a `VideoFrame` with FLV-AVC semantics.
///
/// 生成符合 FLV-AVC 语义的 `VideoFrame`。
fn arb_video_frame() -> impl Strategy<Value = VideoFrame> {
    (
        any::<u32>(),
        -8388608i32..=8388607i32,
        arb_video_frame_type(),
        arb_video_codec(),
        prop::option::of(arb_avc_packet_type()),
        prop::collection::vec(any::<u8>(), 0..64),
    )
        .prop_map(
            |(timestamp, cts_offset, frame_type, codec, avc_packet_type, data)| VideoFrame {
                timestamp: RtmpTimestamp::from_millis(timestamp),
                composition_timestamp_offset: RtmpTimestampDelta::from_millis(cts_offset),
                frame_type,
                codec,
                avc_packet_type,
                data,
            },
        )
}

/// Generate a `MediaFrame` that is either audio or video.
///
/// 生成音频或视频之一的 `MediaFrame`。
fn arb_media_frame() -> impl Strategy<Value = MediaFrame> {
    prop_oneof![
        arb_audio_frame().prop_map(MediaFrame::Audio),
        arb_video_frame().prop_map(MediaFrame::Video),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Verify that `VideoFrame::is_keyframe()` matches the `KeyFrame` variant.
    ///
    /// 校验 `VideoFrame::is_keyframe()` 与 `KeyFrame` 变体一致。
    #[test]
    fn video_frame_is_keyframe(frame in arb_video_frame()) {
        let expected = frame.frame_type == VideoFrameType::KeyFrame;
        prop_assert_eq!(frame.is_keyframe(), expected);
    }

    /// Verify that `MediaFrame::Audio` wraps the original `AudioFrame`.
    ///
    /// 校验 `MediaFrame::Audio` 正确持有原始 `AudioFrame`。
    #[test]
    fn media_frame_audio_variant(audio_frame in arb_audio_frame()) {
        let media = MediaFrame::Audio(audio_frame.clone());
        match media {
            MediaFrame::Audio(inner) => prop_assert_eq!(inner, audio_frame),
            MediaFrame::Video(_) => prop_assert!(false, "Expected Audio variant"),
        }
    }

    /// Verify that `MediaFrame::Video` wraps the original `VideoFrame`.
    ///
    /// 校验 `MediaFrame::Video` 正确持有原始 `VideoFrame`。
    #[test]
    fn media_frame_video_variant(video_frame in arb_video_frame()) {
        let media = MediaFrame::Video(video_frame.clone());
        match media {
            MediaFrame::Video(inner) => prop_assert_eq!(inner, video_frame),
            MediaFrame::Audio(_) => prop_assert!(false, "Expected Video variant"),
        }
    }

    /// Verify that `MediaFrame` clones correctly.
    ///
    /// 校验 `MediaFrame` 正确实现 Clone。
    #[test]
    fn media_frame_clone(frame in arb_media_frame()) {
        let cloned = frame.clone();
        prop_assert_eq!(cloned, frame);
    }

    /// Verify that `AudioFrame` clones correctly.
    ///
    /// 校验 `AudioFrame` 正确实现 Clone。
    #[test]
    fn audio_frame_clone(frame in arb_audio_frame()) {
        let cloned = frame.clone();
        prop_assert_eq!(cloned, frame);
    }

    /// Verify that `VideoFrame` clones correctly.
    ///
    /// 校验 `VideoFrame` 正确实现 Clone。
    #[test]
    fn video_frame_clone(frame in arb_video_frame()) {
        let cloned = frame.clone();
        prop_assert_eq!(cloned, frame);
    }

    /// Verify that `AudioFormat::raw_id()` is in the valid FLV range.
    ///
    /// 校验 `AudioFormat::raw_id()` 在有效 FLV 范围内。
    #[test]
    fn audio_format_values(format in arb_audio_format()) {
        let value = format.raw_id();
        prop_assert!(
            matches!(value, 1..=8 | 10 | 11 | 14 | 15),
            "Invalid audio format value: {}", value
        );
    }

    /// Verify that `AudioSampleRate` is in the 0-3 range.
    ///
    /// 校验 `AudioSampleRate` 在 0-3 范围内。
    #[test]
    fn audio_sample_rate_values(rate in arb_audio_sample_rate()) {
        let value = rate as u8;
        prop_assert!(value <= 3, "Invalid sample rate value: {}", value);
    }

    /// Verify that `VideoCodec::raw_id()` is in the valid FLV range.
    ///
    /// 校验 `VideoCodec::raw_id()` 在有效 FLV 范围内。
    #[test]
    fn video_codec_values(codec in arb_video_codec()) {
        let value = codec.raw_id();
        prop_assert!((1..=7).contains(&value), "Invalid video codec value: {}", value);
    }

    /// Verify that `VideoFrameType` is in the valid 1-5 range.
    ///
    /// 校验 `VideoFrameType` 在有效 1-5 范围内。
    #[test]
    fn video_frame_type_values(frame_type in arb_video_frame_type()) {
        let value = frame_type as u8;
        prop_assert!((1..=5).contains(&value), "Invalid video frame type value: {}", value);
    }

    /// Verify that `AvcPacketType` is in the valid 0-2 range.
    ///
    /// 校验 `AvcPacketType` 在有效 0-2 范围内。
    #[test]
    fn avc_packet_type_values(packet_type in arb_avc_packet_type()) {
        let value = packet_type as u8;
        prop_assert!(value <= 2, "Invalid AVC packet type value: {}", value);
    }
}
