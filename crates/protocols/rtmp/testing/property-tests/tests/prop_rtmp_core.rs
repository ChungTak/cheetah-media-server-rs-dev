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

fn c0c1_packet(c1: &[u8; 1536]) -> Bytes {
    let mut wire = Vec::with_capacity(1537);
    wire.push(3);
    wire.extend_from_slice(c1);
    Bytes::from(wire)
}

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

fn has_connected(outputs: &[CoreOutput], app: &str) -> bool {
    outputs.iter().any(|out| {
        matches!(
            out,
            CoreOutput::Event(RtmpEvent::Connected { app: got, .. }) if got == app
        )
    })
}

fn drive_ready(core: &mut RtmpCore) {
    let c1 = [0u8; 1536];
    core.handle_input(CoreInput::Bytes(c0c1_packet(&c1)))
        .expect("c0c1");
    core.handle_input(CoreInput::Bytes(Bytes::from(vec![0u8; 1536])))
        .expect("c2");
}

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
