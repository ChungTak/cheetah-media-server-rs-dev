//! Targeted branch tests for `RtmpMessageDecoder`.
//!
//! These tests exercise decoder edge cases that are unlikely to appear naturally
//! during generic round-trip property testing, such as the AMF3 zero-prefix fallback
//! for command messages and malformed protocol control payloads.
//!
//! `RtmpMessageDecoder` 的分支测试。
//!
//! 这些测试专门覆盖通用往返属性测试中 unlikely 出现的解码器边界分支，
//! 例如 AMF3 命令消息的零前缀回退以及损坏的协议控制负载。

use cheetah_rtmp_core::ErrorKind;
use cheetah_rtmp_core::{
    Amf0Value, AmfValue, AmfVersion, RtmpChunk, RtmpChunkEncoder, RtmpChunkStreamId, RtmpMessage,
    RtmpMessageDecoder, RtmpMessageStreamId, RtmpMessageType, RtmpTimestamp,
};

use bytes::Bytes;

/// Assemble a single RTMP chunk around the given message type and payload.
///
/// 根据给定 message type 和 payload 组装单个 RTMP chunk。
fn encode_chunk(message_type: RtmpMessageType, payload: Vec<u8>) -> Vec<u8> {
    let chunk = RtmpChunk {
        chunk_stream_id: RtmpChunkStreamId::new(3).unwrap(),
        message_stream_id: RtmpMessageStreamId::PCM,
        message_type,
        timestamp: RtmpTimestamp::from_millis(0),
        payload: Bytes::from(payload),
    };
    let mut encoder = RtmpChunkEncoder::default();
    let mut buf = Vec::new();
    encoder.encode(&mut buf, &chunk);
    buf
}

/// Verify that an AMF3 command message starting with a zero byte is treated as AMF0.
///
/// Some clients send AMF3 command type (20) but prefix the payload with `0x00`
/// before the AMF0-encoded values. The decoder must detect this and fall back to AMF0.
///
/// 校验以零字节开头的 AMF3 命令消息会被当作 AMF0 处理。
///
/// 某些客户端发送 AMF3 命令类型（20），但会在 AMF0 编码值前加 `0x00` 前缀。
/// 解码器必须检测到此情况并回退到 AMF0。
#[test]
fn command_amf3_zero_prefix_treated_as_amf0() {
    let mut payload = Vec::new();
    payload.push(0);
    AmfValue::from((AmfVersion::Amf0, "connect")).encode(&mut payload);
    AmfValue::from((AmfVersion::Amf0, 1.0_f64)).encode(&mut payload);
    AmfValue::Amf0(Amf0Value::Null).encode(&mut payload);

    let buf = encode_chunk(RtmpMessageType::CommandAmf3, payload);
    let mut decoder = RtmpMessageDecoder::default();
    decoder.feed_buf(&buf);
    let message = decoder.decode().unwrap().unwrap();

    match message {
        RtmpMessage::Command {
            amf_version,
            name,
            transaction_id,
            args,
            ..
        } => {
            assert_eq!(amf_version, AmfVersion::Amf0);
            assert_eq!(name, "connect");
            assert_eq!(transaction_id.get(), 1);
            assert!(args.is_empty());
        }
        _ => panic!("unexpected message type"),
    }
}

/// Verify that an invalid `SetPeerBandwidth` limit type is rejected.
///
/// 校验无效的 `SetPeerBandwidth` limit type 会被拒绝。
#[test]
fn set_peer_bandwidth_invalid_limit_type() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&100u32.to_be_bytes());
    payload.push(9);

    let buf = encode_chunk(RtmpMessageType::SetPeerBandwidth, payload);
    let mut decoder = RtmpMessageDecoder::default();
    decoder.feed_buf(&buf);
    let err = decoder
        .decode()
        .expect_err("invalid limit type should error");
    assert_eq!(err.kind, ErrorKind::InvalidData);
}

/// Verify that an unknown user control event type is rejected.
///
/// 校验未知的用户控制事件类型会被拒绝。
#[test]
fn user_control_unknown_event_type() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&999u16.to_be_bytes());

    let buf = encode_chunk(RtmpMessageType::UserControl, payload);
    let mut decoder = RtmpMessageDecoder::default();
    decoder.feed_buf(&buf);
    let err = decoder
        .decode()
        .expect_err("unknown user control event should error");
    assert_eq!(err.kind, ErrorKind::InvalidData);
}

/// Verify that a decoder with insufficient buffered data returns `None`.
///
/// 校验缓冲数据不足时解码器返回 `None`。
#[test]
fn insufficient_buffer_returns_none() {
    let mut decoder = RtmpMessageDecoder::default();
    decoder.feed_buf(&[0]);
    let result = decoder.decode().unwrap();
    assert!(result.is_none());
}
