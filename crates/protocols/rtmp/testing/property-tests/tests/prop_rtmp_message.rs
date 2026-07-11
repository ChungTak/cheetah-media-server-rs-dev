//! Property-based round-trip tests for the RTMP message layer.
//!
//! These tests encode and decode every `RtmpMessage` variant through the public
//! `RtmpMessageEncoder`/`RtmpMessageDecoder` API. Protocol control messages (types
//! 1-6), user control messages (type 4), and command/data/media messages (types
//! 8, 9, 15, 17, 18, 20) are all exercised. NaN values in command/data payloads are
//! skipped because they cannot be compared with `==`.
//!
//! RTMP 消息层属性测试往返测试。
//!
//! 这些测试通过公共 `RtmpMessageEncoder`/`RtmpMessageDecoder` API 对每种 `RtmpMessage` 变体进行编码与解码。
//! 协议控制消息（类型 1-6）、用户控制消息（类型 4）以及命令/数据/媒体消息（类型 8、9、15、17、18、20）
//! 都被覆盖。命令/数据负载中的 NaN 值会被跳过，因为它无法通过 `==` 比较。

use bytes::Bytes;
use cheetah_rtmp_core::{
    Amf0Value, Amf3Value, AmfValue, AmfVersion, Pair, RtmpChunkSize, RtmpChunkStreamId,
    RtmpMessage, RtmpMessageDecoder, RtmpMessageEncoder, RtmpMessageHeader, RtmpMessageStreamId,
    RtmpTimestamp, RtmpTimestampDelta, RtmpUserControlEvent, SetPeerBandwidthLimitType,
    TransactionId,
};
use cheetah_rtmp_core::{
    AudioFormat, AudioFrame, AudioSampleRate, AvcPacketType, VideoCodec, VideoFrame, VideoFrameType,
};
use proptest::collection::vec;
use proptest::prelude::*;

// =============================================================================
// Strategy definitions
// =============================================================================

/// Generate an arbitrary `RtmpMessageHeader`.
///
/// 生成任意 `RtmpMessageHeader`。
fn arb_message_header() -> impl Strategy<Value = RtmpMessageHeader> {
    (any::<u32>(), any::<u32>()).prop_map(|(stream_id, timestamp_ms)| RtmpMessageHeader {
        stream_id: RtmpMessageStreamId::new(stream_id),
        timestamp: RtmpTimestamp::from_millis(timestamp_ms),
    })
}

/// Generate a protocol-control `RtmpMessageHeader` with stream id 0.
///
/// 生成 message stream id 为 0 的协议控制 `RtmpMessageHeader`。
fn arb_pcm_header() -> impl Strategy<Value = RtmpMessageHeader> {
    any::<u32>().prop_map(|timestamp_ms| RtmpMessageHeader {
        stream_id: RtmpMessageStreamId::PCM,
        timestamp: RtmpTimestamp::from_millis(timestamp_ms),
    })
}

/// Generate an arbitrary `RtmpChunkSize`.
///
/// 生成任意 `RtmpChunkSize`。
fn arb_chunk_size() -> impl Strategy<Value = RtmpChunkSize> {
    prop_oneof![
        Just(RtmpChunkSize::new(1).unwrap()),
        Just(RtmpChunkSize::new(128).unwrap()),
        Just(RtmpChunkSize::new(65536).unwrap()),
        (1usize..=65536).prop_filter_map("valid chunk size", RtmpChunkSize::new),
    ]
}

/// Generate an arbitrary `RtmpChunkStreamId`.
///
/// 生成任意 `RtmpChunkStreamId`。
fn arb_chunk_stream_id() -> impl Strategy<Value = RtmpChunkStreamId> {
    prop_oneof![
        Just(RtmpChunkStreamId::new(2).unwrap()),
        Just(RtmpChunkStreamId::new(63).unwrap()),
        Just(RtmpChunkStreamId::new(64).unwrap()),
        Just(RtmpChunkStreamId::new(319).unwrap()),
        Just(RtmpChunkStreamId::new(320).unwrap()),
        Just(RtmpChunkStreamId::new(65599).unwrap()),
        (2u32..=65599).prop_filter_map("valid chunk stream id", RtmpChunkStreamId::new),
    ]
}

/// Generate a `SetPeerBandwidthLimitType`.
///
/// 生成 `SetPeerBandwidthLimitType`。
fn arb_limit_type() -> impl Strategy<Value = SetPeerBandwidthLimitType> {
    prop_oneof![
        Just(SetPeerBandwidthLimitType::Hard),
        Just(SetPeerBandwidthLimitType::Soft),
        Just(SetPeerBandwidthLimitType::Dynamic),
    ]
}

/// Generate an arbitrary `RtmpUserControlEvent`.
///
/// 生成任意 `RtmpUserControlEvent`。
fn arb_user_control_event() -> impl Strategy<Value = RtmpUserControlEvent> {
    prop_oneof![
        any::<u32>().prop_map(|stream_id| RtmpUserControlEvent::StreamBegin {
            stream_id: RtmpMessageStreamId::new(stream_id),
        }),
        any::<u32>().prop_map(|stream_id| RtmpUserControlEvent::StreamEof {
            stream_id: RtmpMessageStreamId::new(stream_id),
        }),
        any::<u32>().prop_map(|stream_id| RtmpUserControlEvent::StreamDry {
            stream_id: RtmpMessageStreamId::new(stream_id),
        }),
        (any::<u32>(), any::<u32>()).prop_map(|(stream_id, length)| {
            RtmpUserControlEvent::SetBufferLength {
                stream_id: RtmpMessageStreamId::new(stream_id),
                length,
            }
        }),
        any::<u32>().prop_map(|stream_id| RtmpUserControlEvent::StreamIsRecorded {
            stream_id: RtmpMessageStreamId::new(stream_id),
        }),
        any::<u32>().prop_map(|ms| RtmpUserControlEvent::PingRequest {
            timestamp: RtmpTimestamp::from_millis(ms)
        }),
        any::<u32>().prop_map(|ms| RtmpUserControlEvent::PingResponse {
            timestamp: RtmpTimestamp::from_millis(ms)
        }),
        any::<u32>().prop_map(|stream_id| RtmpUserControlEvent::BufferEmpty {
            stream_id: RtmpMessageStreamId::new(stream_id),
        }),
        any::<u32>().prop_map(|stream_id| RtmpUserControlEvent::BufferReady {
            stream_id: RtmpMessageStreamId::new(stream_id),
        }),
    ]
}

/// Generate an `AudioFormat`.
///
/// 生成 `AudioFormat`。
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

/// Generate an `AudioFrame` with AAC sequence-header normalization.
///
/// 生成带 AAC 序列头归一化的 `AudioFrame`。
fn arb_audio_frame() -> impl Strategy<Value = AudioFrame> {
    (
        any::<u32>(),
        arb_audio_format(),
        arb_audio_sample_rate(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        vec(any::<u8>(), 0..256),
    )
        .prop_map(
            |(
                timestamp_ms,
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
                    timestamp: RtmpTimestamp::from_millis(timestamp_ms),
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

/// Generate a `VideoFrame` with FLV-AVC semantics.
///
/// `avc_packet_type` and `composition_timestamp_offset` are only meaningful for AVC
/// key/inter frames. They are reset to `None`/`ZERO` for non-AVC codecs and the
/// info/command frame type so the round-trip is stable.
///
/// 生成符合 FLV-AVC 语义的 `VideoFrame`。
///
/// `avc_packet_type` 与 `composition_timestamp_offset` 仅在 AVC 关键帧/间帧时有效。
/// 对于非 AVC 编解码器与 info/command 帧类型，它们被重置为 `None`/`ZERO`，以保证往返稳定。
fn arb_video_frame() -> impl Strategy<Value = VideoFrame> {
    (
        any::<u32>(),
        -8388608i32..=8388607i32,
        arb_video_frame_type(),
        arb_video_codec(),
        prop_oneof![
            Just(AvcPacketType::SequenceHeader),
            Just(AvcPacketType::NalUnit),
            Just(AvcPacketType::EndOfSequence),
        ],
        vec(any::<u8>(), 0..256),
    )
        .prop_map(
            |(timestamp_ms, composition_offset, frame_type, codec, avc_type, data)| {
                let (avc_packet_type, composition_timestamp_offset) = if codec == VideoCodec::Avc
                    && frame_type != VideoFrameType::VideoInfoOrCommandFrame
                {
                    (
                        Some(avc_type),
                        RtmpTimestampDelta::from_millis(composition_offset),
                    )
                } else {
                    (None, RtmpTimestampDelta::ZERO)
                };
                VideoFrame {
                    timestamp: RtmpTimestamp::from_millis(timestamp_ms),
                    composition_timestamp_offset,
                    frame_type,
                    codec,
                    avc_packet_type,
                    data,
                }
            },
        )
}

/// Generate a simple AMF0 value for command/data payloads.
///
/// 生成用于命令/数据负载的简单 AMF0 值。
fn arb_simple_amf0_value() -> impl Strategy<Value = Amf0Value> {
    prop_oneof![
        prop::num::f64::NORMAL.prop_map(Amf0Value::Number),
        Just(Amf0Value::Number(0.0)),
        any::<bool>().prop_map(Amf0Value::Boolean),
        "[a-zA-Z0-9]{0,50}".prop_map(Amf0Value::String),
        Just(Amf0Value::Null),
        Just(Amf0Value::Undefined),
    ]
}

/// Generate a simple AMF3 value for command/data payloads.
///
/// 生成用于命令/数据负载的简单 AMF3 值。
fn arb_simple_amf3_value() -> impl Strategy<Value = Amf3Value> {
    prop_oneof![
        (-268435456i32..=268435455i32).prop_map(Amf3Value::Integer),
        prop::num::f64::NORMAL.prop_map(Amf3Value::Double),
        Just(Amf3Value::Double(0.0)),
        any::<bool>().prop_map(Amf3Value::Boolean),
        "[a-zA-Z0-9]{0,50}".prop_map(Amf3Value::String),
        Just(Amf3Value::Null),
        Just(Amf3Value::Undefined),
    ]
}

/// Generate a simple AMF0 object.
///
/// 生成简单 AMF0 对象。
fn arb_amf0_object() -> impl Strategy<Value = Amf0Value> {
    vec(("[a-zA-Z]{1,20}", arb_simple_amf0_value()), 0..5).prop_map(|entries| Amf0Value::Object {
        class_name: None,
        entries: entries
            .into_iter()
            .map(|(key, value)| Pair { key, value })
            .collect(),
    })
}

/// Generate a simple AMF3 object.
///
/// 生成简单 AMF3 对象。
fn arb_amf3_object() -> impl Strategy<Value = Amf3Value> {
    vec(("[a-zA-Z]{1,20}", arb_simple_amf3_value()), 0..5).prop_map(|entries| {
        let entries: Vec<_> = entries
            .into_iter()
            .map(|(key, value)| Pair { key, value })
            .collect();
        Amf3Value::Object {
            class_name: None,
            sealed_count: 0,
            entries,
        }
    })
}

/// Generate an `AmfValue` wrapping AMF0.
///
/// 生成包装 AMF0 的 `AmfValue`。
fn arb_amf_value_amf0() -> impl Strategy<Value = AmfValue> {
    prop_oneof![
        arb_simple_amf0_value().prop_map(AmfValue::Amf0),
        arb_amf0_object().prop_map(AmfValue::Amf0),
    ]
}

/// Generate an `AmfValue` wrapping AMF3.
///
/// 生成包装 AMF3 的 `AmfValue`。
fn arb_amf_value_amf3() -> impl Strategy<Value = AmfValue> {
    prop_oneof![
        arb_simple_amf3_value().prop_map(AmfValue::Amf3),
        arb_amf3_object().prop_map(AmfValue::Amf3),
    ]
}

// =============================================================================
// Protocol control messages (Section 5.4)
// =============================================================================

/// Generate a `SetChunkSize` message.
///
/// 生成 `SetChunkSize` 消息。
fn arb_set_chunk_size_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_pcm_header(), arb_chunk_size())
        .prop_map(|(header, size)| RtmpMessage::SetChunkSize { header, size })
}

/// Generate an `Abort` message.
///
/// 生成 `Abort` 消息。
fn arb_abort_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_pcm_header(), arb_chunk_stream_id()).prop_map(|(header, chunk_stream_id)| {
        RtmpMessage::Abort {
            header,
            chunk_stream_id,
        }
    })
}

/// Generate an `Ack` message.
///
/// 生成 `Ack` 消息。
fn arb_ack_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_pcm_header(), any::<u32>()).prop_map(|(header, sequence_number)| RtmpMessage::Ack {
        header,
        sequence_number,
    })
}

/// Generate a `WinAckSize` message.
///
/// 生成 `WinAckSize` 消息。
fn arb_win_ack_size_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_pcm_header(), any::<u32>())
        .prop_map(|(header, size)| RtmpMessage::WinAckSize { header, size })
}

/// Generate a `SetPeerBandwidth` message.
///
/// 生成 `SetPeerBandwidth` 消息。
fn arb_set_peer_bandwidth_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_pcm_header(), any::<u32>(), arb_limit_type()).prop_map(|(header, size, limit_type)| {
        RtmpMessage::SetPeerBandwidth {
            header,
            size,
            limit_type,
        }
    })
}

/// Generate a `UserControl` message.
///
/// 生成 `UserControl` 消息。
fn arb_user_control_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_pcm_header(), arb_user_control_event())
        .prop_map(|(header, event)| RtmpMessage::UserControl { header, event })
}

// =============================================================================
// Media messages (Section 7.1.4, 7.1.5)
// =============================================================================

/// Generate an `Audio` message.
///
/// The frame timestamp is forced to match the message header timestamp because the
/// decoder sets the frame timestamp from the header.
///
/// 生成 `Audio` 消息。
///
/// frame timestamp 被强制与 message header timestamp 一致，因为解码器从 header 设置 frame timestamp。
fn arb_audio_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_message_header(), arb_audio_frame()).prop_map(|(header, mut frame)| {
        frame.timestamp = header.timestamp;
        RtmpMessage::Audio {
            header,
            frame,
            payload: Bytes::new(),
        }
    })
}

/// Generate a header for audio/video messages.
///
/// 生成音视频消息 header。
fn arb_media_header() -> impl Strategy<Value = RtmpMessageHeader> {
    (any::<u32>(), any::<u32>()).prop_map(|(stream_id, timestamp_ms)| RtmpMessageHeader {
        stream_id: RtmpMessageStreamId::new(stream_id),
        timestamp: RtmpTimestamp::from_millis(timestamp_ms),
    })
}

/// Generate a `Video` message.
///
/// The frame timestamp is forced to match the header. If `avc_packet_type` is `None`,
/// the composition offset is reset to zero because the decoder does not preserve it.
///
/// 生成 `Video` 消息。
///
/// frame timestamp 被强制与 header 一致。若 `avc_packet_type` 为 `None`，
/// 合成偏移被重置为零，因为解码器不会保留它。
fn arb_video_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_media_header(), arb_video_frame()).prop_map(|(header, mut frame)| {
        frame.timestamp = header.timestamp;
        if frame.avc_packet_type.is_none() {
            frame.composition_timestamp_offset = RtmpTimestampDelta::ZERO;
        }
        RtmpMessage::Video {
            header,
            frame,
            payload: Bytes::new(),
        }
    })
}

// =============================================================================
// Command/Data messages (Section 7.1.1, 7.1.2)
// =============================================================================

/// Generate an AMF0 command message.
///
/// 生成 AMF0 命令消息。
fn arb_command_amf0_message() -> impl Strategy<Value = RtmpMessage> {
    (
        arb_message_header(),
        "[a-zA-Z]{1,20}",
        0i64..=1_000_000i64,
        arb_amf0_object(),
        vec(arb_amf_value_amf0(), 0..3),
    )
        .prop_map(
            |(header, name, transaction_id, object, args)| RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf0,
                name,
                transaction_id: TransactionId::from_f64(transaction_id as f64),
                object: AmfValue::Amf0(object),
                args,
            },
        )
}

/// Generate an AMF3 command message.
///
/// 生成 AMF3 命令消息。
fn arb_command_amf3_message() -> impl Strategy<Value = RtmpMessage> {
    (
        arb_message_header(),
        "[a-zA-Z]{1,20}",
        0i64..=1_000_000i64,
        arb_amf3_object(),
        vec(arb_amf_value_amf3(), 0..3),
    )
        .prop_map(
            |(header, name, transaction_id, object, args)| RtmpMessage::Command {
                header,
                amf_version: AmfVersion::Amf3,
                name,
                transaction_id: TransactionId::from_f64(transaction_id as f64),
                object: AmfValue::Amf3(object),
                args,
            },
        )
}

/// Generate an AMF0 data message.
///
/// 生成 AMF0 数据消息。
fn arb_data_amf0_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_message_header(), vec(arb_amf_value_amf0(), 1..5)).prop_map(|(header, values)| {
        RtmpMessage::Data {
            header,
            amf_version: AmfVersion::Amf0,
            values,
        }
    })
}

/// Generate an AMF3 data message.
///
/// 生成 AMF3 数据消息。
fn arb_data_amf3_message() -> impl Strategy<Value = RtmpMessage> {
    (arb_message_header(), vec(arb_amf_value_amf3(), 1..5)).prop_map(|(header, values)| {
        RtmpMessage::Data {
            header,
            amf_version: AmfVersion::Amf3,
            values,
        }
    })
}

// =============================================================================
// All RTMP messages
// =============================================================================

/// Generate an arbitrary `RtmpMessage` covering all variants.
///
/// 生成覆盖所有变体的任意 `RtmpMessage`。
fn arb_rtmp_message() -> impl Strategy<Value = RtmpMessage> {
    prop_oneof![
        arb_set_chunk_size_message(),
        arb_abort_message(),
        arb_ack_message(),
        arb_win_ack_size_message(),
        arb_set_peer_bandwidth_message(),
        arb_user_control_message(),
        arb_audio_message(),
        arb_video_message(),
        arb_command_amf0_message(),
        arb_command_amf3_message(),
        arb_data_amf0_message(),
        arb_data_amf3_message(),
    ]
}

// =============================================================================
// Round-trip tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Verify round-trip for `SetChunkSize`.
    ///
    /// 校验 `SetChunkSize` 往返。
    #[test]
    fn set_chunk_size_roundtrip(message in arb_set_chunk_size_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for `Abort`.
    ///
    /// 校验 `Abort` 往返。
    #[test]
    fn abort_roundtrip(message in arb_abort_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for `Ack`.
    ///
    /// 校验 `Ack` 往返。
    #[test]
    fn ack_roundtrip(message in arb_ack_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for `WinAckSize`.
    ///
    /// 校验 `WinAckSize` 往返。
    #[test]
    fn win_ack_size_roundtrip(message in arb_win_ack_size_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for `SetPeerBandwidth`.
    ///
    /// 校验 `SetPeerBandwidth` 往返。
    #[test]
    fn set_peer_bandwidth_roundtrip(message in arb_set_peer_bandwidth_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for `UserControl`.
    ///
    /// 校验 `UserControl` 往返。
    #[test]
    fn user_control_roundtrip(message in arb_user_control_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for `Audio`.
    ///
    /// 校验 `Audio` 往返。
    #[test]
    fn audio_roundtrip(message in arb_audio_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for `Video`.
    ///
    /// 校验 `Video` 往返。
    #[test]
    fn video_roundtrip(message in arb_video_message()) {
        roundtrip_test(message)?;
    }

    /// Verify round-trip for AMF0 command messages.
    ///
    /// 校验 AMF0 命令消息往返。
    #[test]
    fn command_amf0_roundtrip(message in arb_command_amf0_message()) {
        if contains_nan(&message) {
            return Ok(());
        }
        roundtrip_test(message)?;
    }

    /// Verify round-trip for AMF3 command messages.
    ///
    /// 校验 AMF3 命令消息往返。
    #[test]
    fn command_amf3_roundtrip(message in arb_command_amf3_message()) {
        if contains_nan(&message) {
            return Ok(());
        }
        roundtrip_test(message)?;
    }

    /// Verify round-trip for AMF0 data messages.
    ///
    /// 校验 AMF0 数据消息往返。
    #[test]
    fn data_amf0_roundtrip(message in arb_data_amf0_message()) {
        if contains_nan(&message) {
            return Ok(());
        }
        roundtrip_test(message)?;
    }

    /// Verify round-trip for AMF3 data messages.
    ///
    /// 校验 AMF3 数据消息往返。
    #[test]
    fn data_amf3_roundtrip(message in arb_data_amf3_message()) {
        if contains_nan(&message) {
            return Ok(());
        }
        roundtrip_test(message)?;
    }

    /// Verify round-trip for an arbitrary message.
    ///
    /// 校验任意消息往返。
    #[test]
    fn all_message_roundtrip(message in arb_rtmp_message()) {
        if contains_nan(&message) {
            return Ok(());
        }
        roundtrip_test(message)?;
    }
}

// =============================================================================
// Boundary tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Verify `SetChunkSize` at boundary values.
    ///
    /// 校验 `SetChunkSize` 边界值。
    #[test]
    fn set_chunk_size_boundary(
        size in prop_oneof![
            Just(1usize),
            Just(128usize),
            Just(65536usize),
        ]
    ) {
        if let Some(chunk_size) = RtmpChunkSize::new(size) {
            let message = RtmpMessage::SetChunkSize {
                header: RtmpMessageHeader {
                    stream_id: RtmpMessageStreamId::PCM,
                    timestamp: RtmpTimestamp::ZERO,
                },
                size: chunk_size,
            };
            roundtrip_test(message)?;
        }
    }

    /// Verify `Abort` at chunk stream id boundary values.
    ///
    /// 校验 `Abort` 在 chunk stream id 边界值。
    #[test]
    fn abort_boundary(
        id in prop_oneof![
            Just(2u32),
            Just(63u32),
            Just(64u32),
            Just(319u32),
            Just(320u32),
            Just(65599u32),
        ]
    ) {
        if let Some(chunk_stream_id) = RtmpChunkStreamId::new(id) {
            let message = RtmpMessage::Abort {
                header: RtmpMessageHeader {
                    stream_id: RtmpMessageStreamId::PCM,
                    timestamp: RtmpTimestamp::ZERO,
                },
                chunk_stream_id,
            };
            roundtrip_test(message)?;
        }
    }

    /// Verify timestamp boundary values encode extended timestamps correctly.
    ///
    /// 校验时间戳边界值正确编码扩展时间戳。
    #[test]
    fn timestamp_boundary(
        timestamp_ms in prop_oneof![
            Just(0u32),
            Just(0xFFFFFEu32),
            Just(0xFFFFFFu32),
            Just(0x1000000u32),
            Just(0xFFFFFFFFu32),
        ]
    ) {
        let message = RtmpMessage::Ack {
            header: RtmpMessageHeader {
                stream_id: RtmpMessageStreamId::PCM,
                timestamp: RtmpTimestamp::from_millis(timestamp_ms),
            },
            sequence_number: 12345,
        };
        roundtrip_test(message)?;
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Encode a message and assert it decodes back to the same value.
///
/// 编码消息并断言其解码回相同值。
fn roundtrip_test(message: RtmpMessage) -> Result<(), TestCaseError> {
    let chunk_stream_id = RtmpChunkStreamId::new(3).unwrap();
    let mut encoder = RtmpMessageEncoder::default();
    let mut buf = Vec::new();

    encoder.encode(&mut buf, chunk_stream_id, message.clone());

    let mut decoder = RtmpMessageDecoder::default();
    decoder.feed_buf(&buf);
    let decoded = decoder
        .decode()
        .map_err(|e| TestCaseError::fail(format!("decode failed: {e:?}")))?
        .ok_or_else(|| TestCaseError::fail("decoded message is None"))?;

    prop_assert_eq!(decoded.header(), message.header(), "header mismatch");
    prop_assert_eq!(
        decoded.message_type(),
        message.message_type(),
        "message type mismatch"
    );

    match (&decoded, &message) {
        (
            RtmpMessage::Audio {
                frame: decoded_frame,
                ..
            },
            RtmpMessage::Audio {
                frame: original_frame,
                ..
            },
        ) => {
            prop_assert_eq!(decoded_frame, original_frame, "audio frame mismatch");
        }
        (
            RtmpMessage::Video {
                frame: decoded_frame,
                ..
            },
            RtmpMessage::Video {
                frame: original_frame,
                ..
            },
        ) => {
            prop_assert_eq!(decoded_frame, original_frame, "video frame mismatch");
        }
        _ => {
            prop_assert_eq!(decoded, message, "roundtrip value mismatch");
        }
    }
    Ok(())
}

/// Check whether an `RtmpMessage` contains a NaN float.
///
/// 检查 `RtmpMessage` 是否包含 NaN 浮点数。
fn contains_nan(message: &RtmpMessage) -> bool {
    match message {
        RtmpMessage::Command { object, args, .. } => {
            contains_nan_amf(object) || args.iter().any(contains_nan_amf)
        }
        RtmpMessage::Data { values, .. } => values.iter().any(contains_nan_amf),
        _ => false,
    }
}

fn contains_nan_amf(value: &AmfValue) -> bool {
    match value {
        AmfValue::Amf0(v) => contains_nan_amf0(v),
        AmfValue::Amf3(v) => contains_nan_amf3(v),
    }
}

fn contains_nan_amf0(value: &Amf0Value) -> bool {
    match value {
        Amf0Value::Number(n) => n.is_nan(),
        Amf0Value::Object { entries, .. } => entries.iter().any(|p| contains_nan_amf0(&p.value)),
        Amf0Value::EcmaArray { entries } => entries.iter().any(|p| contains_nan_amf0(&p.value)),
        Amf0Value::Array { entries } => entries.iter().any(contains_nan_amf0),
        Amf0Value::AvmPlus(v) => contains_nan_amf3(v),
        _ => false,
    }
}

fn contains_nan_amf3(value: &Amf3Value) -> bool {
    match value {
        Amf3Value::Double(n) => n.is_nan(),
        Amf3Value::Array {
            assoc_entries,
            dense_entries,
        } => {
            assoc_entries.iter().any(|p| contains_nan_amf3(&p.value))
                || dense_entries.iter().any(contains_nan_amf3)
        }
        Amf3Value::Object { entries, .. } => entries.iter().any(|p| contains_nan_amf3(&p.value)),
        Amf3Value::DoubleVector { entries, .. } => entries.iter().any(|n| n.is_nan()),
        Amf3Value::ObjectVector { entries, .. } => entries.iter().any(contains_nan_amf3),
        Amf3Value::Dictionary { entries, .. } => entries
            .iter()
            .any(|p| contains_nan_amf3(&p.key) || contains_nan_amf3(&p.value)),
        _ => false,
    }
}
