//! RTMP Message Decoder 的分支测试

use cheetah_rtmp_core::ErrorKind;
use cheetah_rtmp_core::{
    Amf0Value, AmfValue, AmfVersion, RtmpChunk, RtmpChunkEncoder, RtmpChunkStreamId, RtmpMessage,
    RtmpMessageDecoder, RtmpMessageStreamId, RtmpMessageType, RtmpTimestamp,
};

use bytes::Bytes;

/// 从指定的 MessageType 和 Payload 组装块
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

/// 验证 AMF3 Command 以 0 开头时会被当作 AMF0 处理
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

/// 验证 SetPeerBandwidth 的 limit_type 无效值会报错
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

/// 验证 UserControl 的未知 event_type 会报错
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

/// 验证缓冲区不足时返回 None
#[test]
fn insufficient_buffer_returns_none() {
    let mut decoder = RtmpMessageDecoder::default();
    decoder.feed_buf(&[0]);
    let result = decoder.decode().unwrap();
    assert!(result.is_none());
}
