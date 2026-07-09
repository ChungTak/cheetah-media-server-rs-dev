use alloc::string::ToString;
use alloc::vec::Vec;

use bytes::Bytes;

use crate::amf::AmfValue;
use crate::amf::Pair;
use crate::amf0::{encode_all, Amf0Value};
use crate::amf3::Amf3Value;
use crate::chunk::{RtmpChunk, RtmpChunkStreamId};
use crate::message::{RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

use super::super::{
    CoreInput, CoreOutput, HandshakeState, RtmpCore, RtmpCoreCommand, RtmpEvent, RtmpMediaType,
};

#[test]
fn pipelined_media_is_buffered_until_accept_publish() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.connected_app = Some("live".to_string());

    let publish_payload = encode_all(&[
        Amf0Value::String("publish".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::Null,
        Amf0Value::String("camera01".to_string()),
    ]);
    core.on_command_message(1, publish_payload, &mut Vec::new())
        .expect("publish command");

    let mut before_accept = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(6).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::new(1),
            message_type: RtmpMessageType::Video,
            timestamp: RtmpTimestamp::from_millis(33),
            payload: Bytes::from_static(&[0x17, 0x01, 0x00]),
        },
        &mut before_accept,
    )
    .expect("video input");
    assert!(!before_accept
        .iter()
        .any(|v| matches!(v, CoreOutput::Event(RtmpEvent::MediaData { .. }))));

    let after_accept = core
        .handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish {
            stream_id: 1,
        }))
        .expect("accept publish");
    assert!(after_accept.iter().any(|v| matches!(
        v,
        CoreOutput::Event(RtmpEvent::MediaData {
            stream_id: 1,
            timestamp_ms: 33,
            media_type: RtmpMediaType::Video,
            ..
        })
    )));
}

#[test]
fn ignores_flex_message_without_error() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(3).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::new(0),
            message_type: RtmpMessageType::CommandAmf3,
            timestamp: RtmpTimestamp::from_millis(0),
            payload: Bytes::from_static(&[0]),
        },
        &mut Vec::new(),
    )
    .map(|_| ())
    .expect("flex handling");
}

#[test]
fn data_amf3_on_metadata_emits_metadata_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let mut payload = Vec::new();
    AmfValue::Amf3(Amf3Value::String("onMetaData".to_string())).encode(&mut payload);
    AmfValue::Amf3(Amf3Value::Object {
        class_name: None,
        sealed_count: 0,
        entries: vec![Pair {
            key: "width".to_string(),
            value: Amf3Value::Double(1920.0),
        }],
    })
    .encode(&mut payload);

    let mut out = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(6).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::new(1),
            message_type: RtmpMessageType::DataAmf3,
            timestamp: RtmpTimestamp::from_millis(7),
            payload: Bytes::from(payload),
        },
        &mut out,
    )
    .expect("data amf3");

    assert!(out.iter().any(|event| matches!(
        event,
        CoreOutput::Event(RtmpEvent::Metadata { stream_id, values })
            if *stream_id == 1
            && values
                .first()
                .and_then(|value| value.expect_str().ok())
                == Some("onMetaData")
    )));
}

#[test]
fn malformed_data_amf3_is_ignored_without_error() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(6).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::new(1),
            message_type: RtmpMessageType::DataAmf3,
            timestamp: RtmpTimestamp::from_millis(0),
            payload: Bytes::from_static(&[0x0B, 0x01]),
        },
        &mut Vec::new(),
    )
    .map(|_| ())
    .expect("malformed data amf3 handling");
}
