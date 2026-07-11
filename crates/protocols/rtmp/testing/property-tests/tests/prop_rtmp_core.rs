//! Property-based tests for the `RtmpCore` Sans-I/O state machine.
//!
//! These tests exercise the core through the public `CoreInput`/`CoreOutput` API:
//! handshake, connect, createStream, publish, media buffering, and deleteStream.
//! They rely on the chunk and AMF encoders to build realistic byte sequences so
//! the tests stay on the public boundary of the core.
//!
//! `RtmpCore` Sans-I/O 状态机的属性测试。
//!
//! 这些测试通过公共 `CoreInput`/`CoreOutput` API 测试核心：握手、连接、createStream、发布、
//! 媒体缓冲以及 deleteStream。它们依赖 chunk 与 AMF 编码器构建真实字节序列，
//! 从而使测试保持在 core 的公共边界上。

use bytes::Bytes;
use cheetah_rtmp_core::{
    decode_all, encode_all, Amf0Value, ErrorKind, RtmpChunk, RtmpChunkDecoder, RtmpChunkEncoder,
    RtmpChunkSize, RtmpChunkStreamId, RtmpMessageStreamId, RtmpMessageType, RtmpTimestamp,
};
use cheetah_rtmp_core::{
    CoreInput, CoreOutput, RtmpCore, RtmpCoreCommand, RtmpCoreError, RtmpEvent,
};
use proptest::collection::vec;
use proptest::prelude::*;

/// Build a C0+C1 byte packet from a 1536-byte C1 payload.
///
/// C0 is the RTMP version byte (3) followed by the 1536-byte C1 random/time data.
///
/// 从 1536 字节 C1 payload 构建 C0+C1 字节包。
///
/// C0 是 RTMP 版本字节（3），后接 1536 字节 C1 随机/时间数据。
fn c0c1_packet(c1: &[u8; 1536]) -> Bytes {
    let mut wire = Vec::with_capacity(1537);
    wire.push(3);
    wire.extend_from_slice(c1);
    Bytes::from(wire)
}

/// Build a chunk-wrapped AMF0 command byte payload.
///
/// 构建 chunk 封装的 AMF0 命令字节 payload。
fn command_wire(message_stream_id: u32, values: &[Amf0Value]) -> Bytes {
    let payload = encode_all(values);
    encode_chunk_wire(
        3,
        0,
        RtmpMessageType::CommandAmf0,
        message_stream_id,
        payload.as_ref(),
        128,
    )
}

/// Build a chunk-wrapped video byte payload.
///
/// 构建 chunk 封装视频字节 payload。
fn video_wire(message_stream_id: u32, timestamp: u32, payload: Vec<u8>) -> Bytes {
    encode_chunk_wire(
        6,
        timestamp,
        RtmpMessageType::Video,
        message_stream_id,
        &payload,
        128,
    )
}

/// Encode a single `RtmpChunk` into bytes, optionally with a configured chunk size.
///
/// 将单个 `RtmpChunk` 编码为字节，可选使用配置的 chunk 大小。
fn encode_chunk_wire(
    csid: u32,
    timestamp_ms: u32,
    message_type: RtmpMessageType,
    message_stream_id: u32,
    payload: &[u8],
    out_chunk_size: usize,
) -> Bytes {
    let chunk = RtmpChunk {
        chunk_stream_id: RtmpChunkStreamId::new(csid).expect("valid csid"),
        message_stream_id: RtmpMessageStreamId::new(message_stream_id),
        message_type,
        timestamp: RtmpTimestamp::from_millis(timestamp_ms),
        payload: Bytes::from(payload.to_vec()),
    };
    let mut encoder = RtmpChunkEncoder::default();
    encoder.set_chunk_size(RtmpChunkSize::saturating_new(out_chunk_size.max(1)));
    let mut wire = Vec::new();
    encoder.encode(&mut wire, &chunk);
    Bytes::from(wire)
}

/// Incrementally decode a segment of chunk bytes and append completed chunks.
///
/// This helper models TCP segmentation by maintaining a `pending` buffer between
/// calls and only returning when `InsufficientBuffer` is encountered.
///
/// 增量解码一段 chunk 字节并追加完成的 chunk。
///
/// 该 helper 通过维护 `pending` 缓冲区模拟 TCP 分段，仅在遇到 `InsufficientBuffer` 时返回。
fn decode_chunks_segmented(
    decoder: &mut RtmpChunkDecoder,
    pending: &mut Vec<u8>,
    segment: &[u8],
    out: &mut Vec<RtmpChunk>,
) {
    pending.extend_from_slice(segment);
    loop {
        match decoder.decode(pending) {
            Ok((consumed, maybe_chunk)) => {
                pending.drain(..consumed);
                if let Some(chunk) = maybe_chunk {
                    out.push(chunk);
                }
            }
            Err(err) if err.kind == ErrorKind::InsufficientBuffer => break,
            Err(err) => panic!("decode chunk failed: {err:?}"),
        }
    }
}

/// Check whether the core outputs indicate a successful `connect` for the given app.
///
/// 检查 core 输出是否指示对指定 app 的成功 `connect`。
fn has_connected(outputs: &[CoreOutput], app: &str) -> bool {
    outputs.iter().any(|out| {
        matches!(
            out,
            CoreOutput::Event(RtmpEvent::Connected { app: got, .. }) if got == app
        )
    })
}

/// Drive the core through handshake and into the ready state.
///
/// This uses zero-filled C1/C2, which is valid because the current handshake
/// implementation does not verify the random bytes.
///
/// 将 core 驱动通过握手并进入就绪状态。
///
/// 使用零填充 C1/C2，当前握手实现不校验随机字节。
fn drive_ready(core: &mut RtmpCore) {
    let c1 = [0u8; 1536];
    core.handle_input(CoreInput::Bytes(c0c1_packet(&c1)))
        .expect("c0c1");
    core.handle_input(CoreInput::Bytes(Bytes::from(vec![0u8; 1536])))
        .expect("c2");
}

/// Extract the `_result` stream id from `createStream` outputs by decoding written chunks.
///
/// This is needed because the core returns `CoreOutput::Write` chunks, and the
/// stream id is embedded as the fourth AMF value of the `_result` command.
///
/// 通过解码 written chunk 从 `createStream` 输出中提取 `_result` stream id。
///
/// 这是因为 core 返回 `CoreOutput::Write` chunk，而 stream id 嵌入在 `_result` 命令的第四个 AMF 值中。
fn create_stream_result_id(decoder: &mut RtmpChunkDecoder, outputs: &[CoreOutput]) -> Option<f64> {
    for out in outputs {
        if let CoreOutput::Write(bytes) = out {
            let mut pending = bytes.to_vec();
            while !pending.is_empty() {
                match decoder.decode(&pending) {
                    Ok((consumed, maybe_chunk)) => {
                        pending.drain(..consumed);
                        if let Some(chunk) = maybe_chunk {
                            if let Ok(values) = decode_all(&chunk.payload) {
                                if let Some(Amf0Value::String(s)) = values.first() {
                                    if s == "_result" {
                                        return values.get(3).and_then(Amf0Value::as_f64);
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
    None
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 300,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// Verify that a single chunk survives arbitrary TCP segmentation and chunking.
    ///
    /// 校验单个 chunk 在任意 TCP 分段与 chunk 分片下都能被完整解码。
    #[test]
    fn chunk_roundtrip_with_segmented_input(
        csid in 2u32..=65_599,
        timestamp in any::<u32>(),
        message_type in prop_oneof![Just(8u8), Just(9u8), Just(18u8), Just(20u8)],
        message_stream_id in any::<u32>(),
        payload in vec(any::<u8>(), 0..4096),
        out_chunk_size in 1usize..=1024,
        segment_sizes in vec(1usize..=256, 1..40),
    ) {
        let payload_bytes = Bytes::from(payload.clone());
        let wire = encode_chunk_wire(
            csid,
            timestamp,
            RtmpMessageType::from_type_id(message_type).expect("valid message type"),
            message_stream_id,
            payload_bytes.as_ref(),
            out_chunk_size,
        );

        let mut decoder = RtmpChunkDecoder::default();
        decoder.set_chunk_size(RtmpChunkSize::saturating_new(out_chunk_size.max(1)));
        let mut pending = Vec::new();
        let mut decoded = Vec::new();
        let mut offset = 0usize;

        for size in segment_sizes {
            if offset >= wire.len() {
                break;
            }
            let take = size.min(wire.len() - offset);
            let part = &wire[offset..offset + take];
            offset += take;
            decode_chunks_segmented(&mut decoder, &mut pending, part, &mut decoded);
        }

        if offset < wire.len() {
            decode_chunks_segmented(&mut decoder, &mut pending, &wire[offset..], &mut decoded);
        }

        prop_assert_eq!(decoded.len(), 1);
        let message = &decoded[0];
        prop_assert_eq!(message.chunk_stream_id.get(), csid);
        prop_assert_eq!(message.timestamp.as_millis(), timestamp);
        prop_assert_eq!(message.message_type as u8, message_type);
        prop_assert_eq!(message.message_stream_id.get(), message_stream_id);
        prop_assert_eq!(&message.payload, payload_bytes.as_ref());
    }

    /// Verify that C2 and `connect` can be pipelined and still produce a `Connected` event.
    ///
    /// 校验 C2 与 `connect` 可以流水线发送并仍产生 `Connected` 事件。
    #[test]
    fn pipelined_c2_and_connect_emits_connected_event(
        app in "[a-z0-9]{1,12}",
        c1 in vec(any::<u8>(), 1536),
    ) {
        let c1: [u8; 1536] = c1.try_into().expect("fixed c1 len");
        let mut core = RtmpCore::new();

        let out1 = core
            .handle_input(CoreInput::Bytes(c0c1_packet(&c1)))
            .expect("handshake c0c1");
        prop_assert!(out1.iter().any(|out| matches!(out, CoreOutput::Write(_))));

        let connect = command_wire(
            0,
            &[
                Amf0Value::String("connect".to_string()),
                Amf0Value::Number(1.0),
                Amf0Value::object([("app", Amf0Value::String(app.clone()))]),
            ],
        );

        let mut c2_and_connect = vec![0u8; 1536];
        c2_and_connect.extend_from_slice(&connect);
        let out2 = core
            .handle_input(CoreInput::Bytes(Bytes::from(c2_and_connect)))
            .expect("handshake c2+connect");

        prop_assert!(has_connected(&out2, &app));
    }

    /// Verify that a `publish` command emits a `PublishRequested` event.
    ///
    /// 校验 `publish` 命令产生 `PublishRequested` 事件。
    #[test]
    fn publish_command_emits_publish_requested(
        app in "[a-z0-9]{1,12}",
        stream_name in "[a-z0-9_]{1,16}",
        stream_id in 1u32..=256,
        c1 in vec(any::<u8>(), 1536),
    ) {
        let c1: [u8; 1536] = c1.try_into().expect("fixed c1 len");
        let mut core = RtmpCore::new();

        core.handle_input(CoreInput::Bytes(c0c1_packet(&c1))).expect("c0c1");
        core.handle_input(CoreInput::Bytes(Bytes::from(vec![0u8; 1536]))).expect("c2");

        let connect = command_wire(
            0,
            &[
                Amf0Value::String("connect".to_string()),
                Amf0Value::Number(1.0),
                Amf0Value::object([("app", Amf0Value::String(app.clone()))]),
            ],
        );
        let _ = core
            .handle_input(CoreInput::Bytes(connect))
            .expect("connect command");

        let publish = command_wire(
            stream_id,
            &[
                Amf0Value::String("publish".to_string()),
                Amf0Value::Number(2.0),
                Amf0Value::Null,
                Amf0Value::String(stream_name.clone()),
            ],
        );

        let out = core
            .handle_input(CoreInput::Bytes(publish))
            .expect("publish command");

        let publish_event_seen = out.iter().any(|event| {
            matches!(
                event,
                CoreOutput::Event(RtmpEvent::PublishRequested {
                    stream_id: got_stream_id,
                    app: got_app,
                    stream_name: got_stream_name,
                    ..
                })
                if *got_stream_id == stream_id && got_app == &app && got_stream_name == &stream_name
            )
        });
        prop_assert!(publish_event_seen);
    }

    /// Verify that media arriving before a publish is accepted is buffered and flushed after.
    ///
    /// 校验在发布被接受前到达的媒体会被缓冲，在接受后才被刷出。
    #[test]
    fn pending_media_is_flushed_after_accept_publish(
        stream_id in 1u32..=256,
        timestamp_ms in any::<u32>(),
        media_payload in vec(any::<u8>(), 0..1024),
        c1 in vec(any::<u8>(), 1536),
    ) {
        let c1: [u8; 1536] = c1.try_into().expect("fixed c1 len");
        let mut core = RtmpCore::new();

        core.handle_input(CoreInput::Bytes(c0c1_packet(&c1))).expect("c0c1");
        core.handle_input(CoreInput::Bytes(Bytes::from(vec![0u8; 1536]))).expect("c2");

        let connect = command_wire(
            0,
            &[
                Amf0Value::String("connect".to_string()),
                Amf0Value::Number(1.0),
                Amf0Value::object([("app", Amf0Value::String("live".to_string()))]),
            ],
        );
        let _ = core.handle_input(CoreInput::Bytes(connect)).expect("connect");

        let publish = command_wire(
            stream_id,
            &[
                Amf0Value::String("publish".to_string()),
                Amf0Value::Number(2.0),
                Amf0Value::Null,
                Amf0Value::String("stream".to_string()),
            ],
        );
        let _ = core.handle_input(CoreInput::Bytes(publish)).expect("publish");

        let media_wire = video_wire(stream_id, timestamp_ms, media_payload.clone());
        let out_before = core
            .handle_input(CoreInput::Bytes(media_wire))
            .expect("media before accept");
        let media_before_accept = out_before
            .iter()
            .any(|event| matches!(event, CoreOutput::Event(RtmpEvent::MediaData { .. })));
        prop_assert!(!media_before_accept);

        let out_after = core
            .handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish { stream_id }))
            .expect("accept publish");

        let media_after_accept = out_after.iter().any(|event| {
            matches!(
                event,
                CoreOutput::Event(RtmpEvent::MediaData {
                    stream_id: got_stream_id,
                    timestamp_ms: got_ts,
                    media_type: cheetah_rtmp_core::RtmpMediaType::Video,
                    payload,
                })
                if *got_stream_id == stream_id
                    && *got_ts == timestamp_ms
                    && payload.as_ref() == media_payload.as_slice()
            )
        });
        prop_assert!(media_after_accept);
    }

    /// Verify that an invalid handshake version is rejected.
    ///
    /// 校验无效的握手版本被拒绝。
    #[test]
    fn invalid_handshake_version_is_rejected(
        invalid_version in any::<u8>().prop_filter("must not equal 3", |v| *v != 3)
    ) {
        let mut c0c1 = vec![0u8; 1537];
        c0c1[0] = invalid_version;

        let mut core = RtmpCore::new();
        let err = core
            .handle_input(CoreInput::Bytes(Bytes::from(c0c1)))
            .expect_err("invalid version must fail");

        prop_assert!(matches!(
            err,
            RtmpCoreError::InvalidHandshakeVersion(v) if v == invalid_version
        ));
    }

    /// Verify that `createStream` returns monotonically increasing stream ids.
    ///
    /// 校验 `createStream` 返回单调递增的 stream id。
    #[test]
    fn create_stream_ids_are_monotonic_per_connection(
        txns in vec(0u16..=2000, 1..24)
    ) {
        let mut core = RtmpCore::new();
        drive_ready(&mut core);

        let mut decoder = RtmpChunkDecoder::default();
        for (idx, txn) in txns.iter().enumerate() {
            let create = command_wire(
                0,
                &[
                    Amf0Value::String("createStream".to_string()),
                    Amf0Value::Number(*txn as f64),
                    Amf0Value::Null,
                ],
            );
            let out = core
                .handle_input(CoreInput::Bytes(create))
                .expect("createStream");
            let stream_id = create_stream_result_id(&mut decoder, &out).expect("stream id result");
            prop_assert_eq!(stream_id, (idx + 1) as f64);
        }
    }

    /// Verify that `deleteStream` uses the argument stream id, not the message stream id.
    ///
    /// 校验 `deleteStream` 使用参数中的 stream id，而非 message stream id。
    #[test]
    fn delete_stream_argument_takes_precedence_over_message_stream_id(
        message_stream_id in 1u32..=64,
        target_stream_id in 1u32..=64,
    ) {
        prop_assume!(message_stream_id != target_stream_id);
        let mut core = RtmpCore::new();
        drive_ready(&mut core);

        let publish = command_wire(
            target_stream_id,
            &[
                Amf0Value::String("publish".to_string()),
                Amf0Value::Number(1.0),
                Amf0Value::Null,
                Amf0Value::String("stream".to_string()),
            ],
        );
        let _ = core.handle_input(CoreInput::Bytes(publish)).expect("publish");

        let _ = core
            .handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish {
                stream_id: target_stream_id,
            }))
            .expect("accept publish");

        let delete = command_wire(
            message_stream_id,
            &[
                Amf0Value::String("deleteStream".to_string()),
                Amf0Value::Number(2.0),
                Amf0Value::Null,
                Amf0Value::Number(target_stream_id as f64),
            ],
        );
        let out = core
            .handle_input(CoreInput::Bytes(delete))
            .expect("deleteStream");

        let closed_target = out.iter().any(|event| {
            matches!(
                event,
                CoreOutput::Event(RtmpEvent::StreamClosed { stream_id }) if *stream_id == target_stream_id
            )
        });
        prop_assert!(closed_target);
    }
}
