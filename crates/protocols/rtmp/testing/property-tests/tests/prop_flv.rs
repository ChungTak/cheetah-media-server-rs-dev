//! Property-based round-trip tests for FLV-style audio and video frames.
//!
//! RTMP audio/video messages are FLV-tagged frames. These tests verify that the
//! encoder and decoder are inverses while respecting the codec-dependent semantics:
//! AAC sequence headers are only meaningful when the audio format is AAC, and
//! AVC packet types / composition offsets are only meaningful for AVC key/inter frames.
//!
//! FLV 风格音视频帧的属性测试往返测试。
//!
//! RTMP 音视频消息是 FLV 标记帧。这些测试校验编码器与解码器互逆，并遵循编解码器相关语义：
//! AAC 序列头仅在音频格式为 AAC 时有效；AVC 包类型与合成偏移仅在 AVC 关键帧/间帧时有效。

use cheetah_rtmp_core::{
    decode_audio_frame, decode_video_frame, encode_audio_frame, encode_video_frame, AudioFormat,
    AudioFrame, AudioSampleRate, AvcPacketType, RtmpTimestamp, RtmpTimestampDelta, VideoCodec,
    VideoFrame, VideoFrameType,
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

/// Generate an `AudioFrame` with codec-dependent sequence-header normalization.
///
/// For non-AAC formats the `is_aac_sequence_header` flag is ignored by the decoder,
/// so the generator forces it to `false` to keep the round-trip invariant stable.
///
/// 生成带编解码器相关序列头归一化的 `AudioFrame`。
///
/// 对于非 AAC 格式，解码器会忽略 `is_aac_sequence_header` 标志，
/// 因此生成器强制将其设为 `false`，以保持往返不变量稳定。
fn arb_audio_frame() -> impl Strategy<Value = AudioFrame> {
    (
        any::<u32>(),
        arb_audio_format(),
        arb_audio_sample_rate(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop::collection::vec(any::<u8>(), 0..256),
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
            )| {
                let is_aac_sequence_header = if format == AudioFormat::Aac {
                    is_aac_sequence_header
                } else {
                    false
                };
                AudioFrame {
                    timestamp: RtmpTimestamp::from_millis(timestamp),
                    format,
                    sample_rate,
                    is_8bit_sample,
                    is_stereo,
                    is_aac_sequence_header,
                    data,
                }
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
/// Only the AVC codec emits `avc_packet_type` and `composition_timestamp_offset`.
/// For non-AVC codecs and `VideoInfoOrCommandFrame` these fields are reset to `None`
/// and `ZERO` so the decoder round-trips exactly.
///
/// 生成符合 FLV-AVC 语义的 `VideoFrame`。
///
/// 仅 AVC 编解码器会产生 `avc_packet_type` 与 `composition_timestamp_offset`。
/// 对于非 AVC 编解码器以及 `VideoInfoOrCommandFrame`，这些字段被重置为 `None` 与 `ZERO`，
/// 使解码器精确往返。
fn arb_video_frame() -> impl Strategy<Value = VideoFrame> {
    (
        any::<u32>(),
        -8388608i32..=8388607i32,
        arb_video_frame_type(),
        arb_video_codec(),
        prop::option::of(arb_avc_packet_type()),
        prop::collection::vec(any::<u8>(), 0..256),
    )
        .prop_map(
            |(timestamp, cts_offset, frame_type, codec, avc_packet_type, data)| {
                let (avc_packet_type, composition_timestamp_offset) = if codec == VideoCodec::Avc
                    && frame_type != VideoFrameType::VideoInfoOrCommandFrame
                {
                    (
                        avc_packet_type.or(Some(AvcPacketType::NalUnit)),
                        RtmpTimestampDelta::from_millis(cts_offset),
                    )
                } else {
                    (None, RtmpTimestampDelta::ZERO)
                };
                VideoFrame {
                    timestamp: RtmpTimestamp::from_millis(timestamp),
                    composition_timestamp_offset,
                    frame_type,
                    codec,
                    avc_packet_type,
                    data,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Verify that `AudioFrame` encoding and decoding are inverses.
    ///
    /// 校验 `AudioFrame` 的编码与解码互逆。
    #[test]
    fn audio_frame_roundtrip(frame in arb_audio_frame()) {
        let mut buf = Vec::new();
        encode_audio_frame(&mut buf, &frame);
        let decoded = decode_audio_frame(&buf, frame.timestamp).expect("decode should succeed");
        prop_assert_eq!(decoded, frame);
    }

    /// Verify that `VideoFrame` encoding and decoding are inverses.
    ///
    /// 校验 `VideoFrame` 的编码与解码互逆。
    #[test]
    fn video_frame_roundtrip(frame in arb_video_frame()) {
        let mut buf = Vec::new();
        encode_video_frame(&mut buf, &frame);
        let decoded = decode_video_frame(&buf, frame.timestamp).expect("decode should succeed");
        prop_assert_eq!(decoded, frame);
    }
}
