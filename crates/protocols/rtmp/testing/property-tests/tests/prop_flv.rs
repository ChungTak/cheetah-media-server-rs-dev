//! FLV (Audio/Video Frame) 的 Property-Based Testing

use cheetah_rtmp_core::{
    decode_audio_frame, decode_video_frame, encode_audio_frame, encode_video_frame, AudioFormat,
    AudioFrame, AudioSampleRate, AvcPacketType, RtmpTimestamp, RtmpTimestampDelta, VideoCodec,
    VideoFrame, VideoFrameType,
};
use proptest::prelude::*;

/// 生成 AudioFormat
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

/// 生成 AudioSampleRate
fn arb_audio_sample_rate() -> impl Strategy<Value = AudioSampleRate> {
    prop_oneof![
        Just(AudioSampleRate::Khz5),
        Just(AudioSampleRate::Khz11),
        Just(AudioSampleRate::Khz22),
        Just(AudioSampleRate::Khz44),
    ]
}

/// 生成 AudioFrame
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
                // AAC 以外的情况 is_aac_sequence_header 会被忽略 (解码时为 false)
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

/// 生成 VideoFrameType
fn arb_video_frame_type() -> impl Strategy<Value = VideoFrameType> {
    prop_oneof![
        Just(VideoFrameType::KeyFrame),
        Just(VideoFrameType::InterFrame),
        Just(VideoFrameType::DisposableInterFrame),
        Just(VideoFrameType::GeneratedKeyFrame),
        Just(VideoFrameType::VideoInfoOrCommandFrame),
    ]
}

/// 生成 VideoCodec
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

/// 生成 AvcPacketType
fn arb_avc_packet_type() -> impl Strategy<Value = AvcPacketType> {
    prop_oneof![
        Just(AvcPacketType::SequenceHeader),
        Just(AvcPacketType::NalUnit),
        Just(AvcPacketType::EndOfSequence),
    ]
}

/// 生成 VideoFrame
fn arb_video_frame() -> impl Strategy<Value = VideoFrame> {
    (
        any::<u32>(),
        // composition_timestamp_offset 是 i24 (有符号24位) 因此限制范围
        -8388608i32..=8388607i32,
        arb_video_frame_type(),
        arb_video_codec(),
        prop::option::of(arb_avc_packet_type()),
        prop::collection::vec(any::<u8>(), 0..256),
    )
        .prop_map(
            |(timestamp, cts_offset, frame_type, codec, avc_packet_type, data)| {
                // 仅在 AVC 编解码器且非 VideoInfoOrCommandFrame 时使用 avc_packet_type 和 cts_offset
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

    /// 验证 AudioFrame 的 encode / decode 是可逆的
    #[test]
    fn audio_frame_roundtrip(frame in arb_audio_frame()) {
        let mut buf = Vec::new();
        encode_audio_frame(&mut buf, &frame);
        let decoded = decode_audio_frame(&buf, frame.timestamp).expect("decode should succeed");
        prop_assert_eq!(decoded, frame);
    }

    /// 验证 VideoFrame 的 encode / decode 是可逆的
    #[test]
    fn video_frame_roundtrip(frame in arb_video_frame()) {
        let mut buf = Vec::new();
        encode_video_frame(&mut buf, &frame);
        let decoded = decode_video_frame(&buf, frame.timestamp).expect("decode should succeed");
        prop_assert_eq!(decoded, frame);
    }
}
