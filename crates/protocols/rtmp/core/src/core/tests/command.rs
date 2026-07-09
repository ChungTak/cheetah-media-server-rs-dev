use alloc::string::ToString;
use alloc::vec::Vec;

use crate::amf0::{decode_all, encode_all, Amf0Value};
use crate::chunk::{RtmpChunk, RtmpChunkDecoder, RtmpChunkEncoder, RtmpChunkStreamId};
use crate::error::ErrorKind;
use crate::message::{RtmpMessage, RtmpMessageHeader, RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;
use crate::user_control::RtmpUserControlEvent;
use bytes::Bytes;

use super::super::{CoreInput, CoreOutput, HandshakeState, RtmpCore, RtmpCoreCommand, RtmpEvent};
use super::decode_first_message;

#[test]
fn command_accept_publish_produces_status() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    let out = core
        .handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPublish {
            stream_id: 1,
        }))
        .expect("command");
    assert!(out.iter().any(|v| matches!(v, CoreOutput::Write(_))));
}

#[test]
fn publish_command_emits_publish_requested_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.connected_app = Some("live".to_string());
    let mut out = Vec::new();
    let payload = encode_all(&[
        Amf0Value::String("publish".to_string()),
        Amf0Value::Number(0.0),
        Amf0Value::Null,
        Amf0Value::String("camera01".to_string()),
    ]);
    core.on_command_message(1, payload, &mut out)
        .expect("publish command");
    assert!(out.iter().any(|v| matches!(
        v,
        CoreOutput::Event(RtmpEvent::PublishRequested {
            stream_id: 1,
            app,
            stream_name,
            ..
        }) if app == "live" && stream_name == "camera01"
    )));
}

#[test]
fn fcpublish_does_not_emit_publish_requested_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.connected_app = Some("live".to_string());
    let mut out = Vec::new();
    let payload = encode_all(&[
        Amf0Value::String("FCPublish".to_string()),
        Amf0Value::Number(0.0),
        Amf0Value::Null,
        Amf0Value::String("camera01".to_string()),
    ]);
    core.on_command_message(1, payload, &mut out)
        .expect("fcpublish command");
    assert!(!out
        .iter()
        .any(|v| matches!(v, CoreOutput::Event(RtmpEvent::PublishRequested { .. }))));
}

#[test]
fn notify_metadata_is_exposed() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    let payload = encode_all(&[
        Amf0Value::String("@setDataFrame".to_string()),
        Amf0Value::String("onMetaData".to_string()),
        Amf0Value::empty_object(),
    ]);
    let event = core
        .on_notify_message(1, payload)
        .expect("parse")
        .expect("event");
    assert!(matches!(event, RtmpEvent::Metadata { .. }));
}

#[test]
fn create_stream_returns_unique_stream_ids_per_connection() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let payload_1 = encode_all(&[
        Amf0Value::String("createStream".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::Null,
    ]);
    let payload_2 = encode_all(&[
        Amf0Value::String("createStream".to_string()),
        Amf0Value::Number(2.0),
        Amf0Value::Null,
    ]);

    let mut decoder = RtmpChunkDecoder::default();

    let mut out_1 = Vec::new();
    core.on_command_message(0, payload_1, &mut out_1)
        .expect("first createStream");
    let chunk_1 = decode_first_message(&mut decoder, &out_1);
    let values_1 = decode_all(&chunk_1.payload).expect("decode amf payload");
    let stream_id_1 = values_1
        .get(3)
        .and_then(Amf0Value::as_f64)
        .expect("stream id number");

    let mut out_2 = Vec::new();
    core.on_command_message(0, payload_2, &mut out_2)
        .expect("second createStream");
    let chunk_2 = decode_first_message(&mut decoder, &out_2);
    let values_2 = decode_all(&chunk_2.payload).expect("decode amf payload");
    let stream_id_2 = values_2
        .get(3)
        .and_then(Amf0Value::as_f64)
        .expect("stream id number");

    assert_eq!(stream_id_1, 1.0);
    assert_eq!(stream_id_2, 2.0);
    assert_ne!(stream_id_1, stream_id_2);
}

#[test]
fn delete_stream_uses_command_argument_stream_id() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.active_publish = Some(7);
    core.pending_publish = Some(7);

    let payload = encode_all(&[
        Amf0Value::String("deleteStream".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::Null,
        Amf0Value::Number(7.0),
    ]);

    let mut out = Vec::new();
    core.on_command_message(0, payload, &mut out)
        .expect("deleteStream command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::StreamClosed { stream_id: 7 })
    )));
    assert_eq!(core.active_publish, None);
    assert_eq!(core.pending_publish, None);
}

#[test]
fn close_stream_falls_back_to_message_stream_id_without_argument() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.active_publish = Some(3);
    core.pending_publish = Some(3);

    let payload = encode_all(&[
        Amf0Value::String("closeStream".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::Null,
    ]);

    let mut out = Vec::new();
    core.on_command_message(3, payload, &mut out)
        .expect("closeStream command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::StreamClosed { stream_id: 3 })
    )));
    assert_eq!(core.active_publish, None);
    assert_eq!(core.pending_publish, None);
}

#[test]
fn close_stream_with_argument_uses_argument_stream_id() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.active_publish = Some(7);
    core.pending_publish = Some(7);

    let payload = encode_all(&[
        Amf0Value::String("closeStream".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::Null,
        Amf0Value::Number(7.0),
    ]);

    let mut out = Vec::new();
    core.on_command_message(3, payload, &mut out)
        .expect("closeStream command with argument");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::StreamClosed { stream_id: 7 })
    )));
    assert_eq!(core.active_publish, None);
    assert_eq!(core.pending_publish, None);
}

#[test]
fn play2_falls_back_to_play_requested_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.connected_app = Some("live".to_string());

    let payload = encode_all(&[
        Amf0Value::String("play2".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::Null,
        Amf0Value::String("stream_alt".to_string()),
    ]);

    let mut out = Vec::new();
    core.on_command_message(1, payload, &mut out)
        .expect("play2 command");

    assert!(out.iter().any(|v| matches!(
        v,
        CoreOutput::Event(RtmpEvent::PlayRequested {
            stream_id: 1,
            app,
            stream_name,
            ..
        }) if app == "live" && stream_name == "stream_alt"
    )));
}

#[test]
fn accept_play_emits_stream_begin_user_control() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    let out = core
        .handle_input(CoreInput::Command(RtmpCoreCommand::AcceptPlay {
            stream_id: 9,
        }))
        .expect("accept play");

    let mut decoder = RtmpChunkDecoder::default();
    let mut pending = Vec::new();
    let mut saw_stream_begin = false;
    for output in out {
        let CoreOutput::Write(bytes) = output else {
            continue;
        };
        pending.extend_from_slice(&bytes);
        loop {
            match decoder.decode(&pending) {
                Ok((consumed, maybe_chunk)) => {
                    pending.drain(..consumed);
                    let Some(chunk) = maybe_chunk else {
                        continue;
                    };
                    if chunk.message_type != RtmpMessageType::UserControl || chunk.payload.len() < 6
                    {
                        continue;
                    }
                    let event_type = u16::from_be_bytes([chunk.payload[0], chunk.payload[1]]);
                    let value = u32::from_be_bytes([
                        chunk.payload[2],
                        chunk.payload[3],
                        chunk.payload[4],
                        chunk.payload[5],
                    ]);
                    if event_type == 0 && value == 9 {
                        saw_stream_begin = true;
                    }
                }
                Err(err) if err.kind == ErrorKind::InsufficientBuffer => break,
                Err(err) => panic!("decode chunk failed: {err:?}"),
            }
        }
    }
    assert!(saw_stream_begin);
}

#[test]
fn flex_command_with_amf0_prefix_emits_connected_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let connect_payload = encode_all(&[
        Amf0Value::String("connect".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::object([("app", Amf0Value::String("live".to_string()))]),
    ]);

    let mut flex_payload = Vec::with_capacity(connect_payload.len() + 1);
    flex_payload.push(0);
    flex_payload.extend_from_slice(&connect_payload);

    let mut out = Vec::new();
    core.on_message(
        crate::chunk::RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(3).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::new(0),
            message_type: RtmpMessageType::CommandAmf3,
            timestamp: RtmpTimestamp::from_millis(0),
            payload: Bytes::from(flex_payload),
        },
        &mut out,
    )
    .expect("flex command");

    assert!(out.iter().any(|event| matches!(
        event,
        CoreOutput::Event(RtmpEvent::Connected { app, .. }) if app == "live"
    )));
}

#[test]
fn set_peer_bandwidth_hard_limit_replies_with_peer_size() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let mut out = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::SetPeerBandwidth,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x01, 0x86, 0xA0, 0x00]),
        },
        &mut out,
    )
    .expect("setPeerBandwidth");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::LocalAckWindowUpdated { size: 100_000 })
    )));
    let mut decoder = RtmpChunkDecoder::default();
    let reply = decode_first_message(&mut decoder, &out);
    let reply = crate::message::decode_rtmp_chunk_to_message(reply).expect("decode message");
    match reply {
        RtmpMessage::WinAckSize { size, .. } => assert_eq!(size, 100_000),
        other => panic!("expected WinAckSize, got {other:?}"),
    }
}

#[test]
fn set_peer_bandwidth_soft_limit_uses_min_of_local_and_peer() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let mut out = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::SetPeerBandwidth,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x01, 0x86, 0xA0, 0x01]),
        },
        &mut out,
    )
    .expect("setPeerBandwidth soft");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::LocalAckWindowUpdated { size: 100_000 })
    )));
    let mut decoder = RtmpChunkDecoder::default();
    let reply = decode_first_message(&mut decoder, &out);
    let reply = crate::message::decode_rtmp_chunk_to_message(reply).expect("decode message");
    match reply {
        RtmpMessage::WinAckSize { size, .. } => assert_eq!(size, 100_000),
        other => panic!("expected WinAckSize, got {other:?}"),
    }
}

#[test]
fn set_peer_bandwidth_dynamic_after_hard_treated_as_hard() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let mut out1 = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::SetPeerBandwidth,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x00, 0x10, 0x00, 0x00]),
        },
        &mut out1,
    )
    .expect("setPeerBandwidth hard first");

    let mut out2 = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::SetPeerBandwidth,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x01, 0x86, 0xA0, 0x02]),
        },
        &mut out2,
    )
    .expect("setPeerBandwidth dynamic after hard");

    let mut decoder = RtmpChunkDecoder::default();
    let _ = decode_first_message(&mut decoder, &out1);
    let reply_chunk = decode_first_message(&mut decoder, &out2);
    let reply = crate::message::decode_rtmp_chunk_to_message(reply_chunk).expect("decode message");
    match reply {
        RtmpMessage::WinAckSize { size, .. } => assert_eq!(size, 100_000),
        other => panic!("expected WinAckSize, got {other:?}"),
    }
}

#[test]
fn set_peer_bandwidth_dynamic_after_soft_treated_as_soft() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let mut out1 = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::SetPeerBandwidth,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x00, 0x10, 0x00, 0x01]),
        },
        &mut out1,
    )
    .expect("setPeerBandwidth soft first");

    let mut out2 = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::SetPeerBandwidth,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x01, 0x86, 0xA0, 0x02]),
        },
        &mut out2,
    )
    .expect("setPeerBandwidth dynamic after soft");

    let mut decoder = RtmpChunkDecoder::default();
    let _ = decode_first_message(&mut decoder, &out1);
    let reply_chunk = decode_first_message(&mut decoder, &out2);
    let reply = crate::message::decode_rtmp_chunk_to_message(reply_chunk).expect("decode message");
    match reply {
        RtmpMessage::WinAckSize { size, .. } => assert_eq!(size, 100_000),
        other => panic!("expected WinAckSize, got {other:?}"),
    }
}

#[test]
fn ack_message_emits_ack_received_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let mut out = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::Ack,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x00, 0x04, 0xD2]),
        },
        &mut out,
    )
    .expect("ack");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::AckReceived {
            sequence_number: 1234
        })
    )));
}

#[test]
fn win_ack_size_message_updates_peer_window_and_emits_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let mut out = Vec::new();
    core.on_message(
        RtmpChunk {
            chunk_stream_id: RtmpChunkStreamId::new(2).expect("valid csid"),
            message_stream_id: RtmpMessageStreamId::PCM,
            message_type: RtmpMessageType::WinAckSize,
            timestamp: RtmpTimestamp::ZERO,
            payload: Bytes::from_static(&[0x00, 0x00, 0x10, 0x00]),
        },
        &mut out,
    )
    .expect("win ack size");

    assert_eq!(core.peer_ack_window_size, 4096);
    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::PeerAckWindowUpdated { size: 4096 })
    )));
}

#[test]
fn ready_input_emits_ack_when_peer_window_exceeded() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;
    core.peer_ack_window_size = 8;

    let chunk = RtmpChunk {
        chunk_stream_id: RtmpChunkStreamId::new(4).expect("valid csid"),
        message_stream_id: RtmpMessageStreamId::new(1),
        message_type: RtmpMessageType::Audio,
        timestamp: RtmpTimestamp::from_millis(0),
        payload: Bytes::from_static(&[0x00, 0x11, 0x22, 0x33, 0x44]),
    };
    let mut wire = Vec::new();
    RtmpChunkEncoder::default().encode(&mut wire, &chunk);

    let out = core
        .handle_input(CoreInput::Bytes(Bytes::from(wire)))
        .expect("ready bytes");
    assert!(out
        .iter()
        .any(|output| matches!(output, CoreOutput::Write(_))));
}

#[test]
fn untracked_result_command_emits_command_ignored_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let payload = encode_all(&[
        Amf0Value::String("_result".to_string()),
        Amf0Value::Number(77.0),
        Amf0Value::object([
            (
                "description",
                Amf0Value::String("Connection succeeded.".to_string()),
            ),
            (
                "code",
                Amf0Value::String("NetConnection.Connect.Success".to_string()),
            ),
        ]),
        Amf0Value::Null,
    ]);

    let mut out = Vec::new();
    core.on_command_message(0, payload, &mut out)
        .expect("result command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::CommandIgnored { name, detail })
            if name == "_result" && detail.contains("unhandled transaction id: 77")
    )));
}

#[test]
fn on_status_without_pending_action_emits_command_ignored_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let payload = encode_all(&[
        Amf0Value::String("onStatus".to_string()),
        Amf0Value::Number(0.0),
        Amf0Value::Null,
        Amf0Value::object([
            ("level", Amf0Value::String("status".to_string())),
            (
                "code",
                Amf0Value::String("NetStream.Play.Start".to_string()),
            ),
            (
                "description",
                Amf0Value::String("Started playing.".to_string()),
            ),
        ]),
    ]);

    let mut out = Vec::new();
    core.on_command_message(1, payload, &mut out)
        .expect("onStatus command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::CommandIgnored { name, detail })
            if name == "onStatus" && detail == "level=status, code=NetStream.Play.Start"
    )));
}

#[test]
fn client_connect_result_emits_client_connected_state_change() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let payload = encode_all(&[
        Amf0Value::String("_result".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::Null,
        Amf0Value::Null,
    ]);

    let mut out = Vec::new();
    core.on_command_message(0, payload, &mut out)
        .expect("connect result command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::ClientStateChanged {
            state: crate::core::RtmpClientState::Connected
        })
    )));
}

#[test]
fn client_create_stream_result_emits_media_stream_created_state_change_for_tracked_tx() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    core.handle_input(CoreInput::Command(RtmpCoreCommand::ClientCreateStream {
        transaction_id: 7.0,
    }))
    .expect("client create stream request");

    let payload = encode_all(&[
        Amf0Value::String("_result".to_string()),
        Amf0Value::Number(7.0),
        Amf0Value::Null,
        Amf0Value::Number(1.0),
    ]);

    let mut out = Vec::new();
    core.on_command_message(0, payload, &mut out)
        .expect("create stream result command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::ClientStateChanged {
            state: crate::core::RtmpClientState::MediaStreamCreated
        })
    )));
}

#[test]
fn client_publish_start_on_status_emits_publishing_state_change() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    core.handle_input(CoreInput::Command(RtmpCoreCommand::ClientPublish {
        stream_id: 1,
        transaction_id: 2.0,
        stream_name: "camera01".to_string(),
    }))
    .expect("client publish request");

    let payload = encode_all(&[
        Amf0Value::String("onStatus".to_string()),
        Amf0Value::Number(0.0),
        Amf0Value::Null,
        Amf0Value::object([
            ("level", Amf0Value::String("status".to_string())),
            (
                "code",
                Amf0Value::String("NetStream.Publish.Start".to_string()),
            ),
        ]),
    ]);

    let mut out = Vec::new();
    core.on_command_message(1, payload, &mut out)
        .expect("publish onStatus command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::ClientStateChanged {
            state: crate::core::RtmpClientState::Publishing
        })
    )));
}

#[test]
fn client_on_status_error_emits_disconnect_requested() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let payload = encode_all(&[
        Amf0Value::String("onStatus".to_string()),
        Amf0Value::Number(0.0),
        Amf0Value::Null,
        Amf0Value::object([
            ("level", Amf0Value::String("error".to_string())),
            (
                "code",
                Amf0Value::String("NetStream.Publish.BadName".to_string()),
            ),
            ("description", Amf0Value::String("in use".to_string())),
            ("details", Amf0Value::String("stream exists".to_string())),
        ]),
    ]);

    let mut out = Vec::new();
    core.on_command_message(1, payload, &mut out)
        .expect("error onStatus command");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::ClientDisconnectRequested { reason })
            if reason == "OnStatus error: NetStream.Publish.BadName - in use (stream exists)"
    )));
}

#[test]
fn client_observe_ack_command_emits_ack_received_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let out = core
        .handle_input(CoreInput::Command(RtmpCoreCommand::ClientObserveAck {
            sequence_number: 42,
        }))
        .expect("client observe ack");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::AckReceived {
            sequence_number: 42
        })
    )));
}

#[test]
fn client_observe_win_ack_size_command_emits_peer_ack_window_updated_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let out = core
        .handle_input(CoreInput::Command(
            RtmpCoreCommand::ClientObserveWinAckSize { size: 65_536 },
        ))
        .expect("client observe win ack size");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::PeerAckWindowUpdated { size: 65_536 })
    )));
}

#[test]
fn client_handle_set_peer_bandwidth_command_emits_local_window_update_and_reply() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let out = core
        .handle_input(CoreInput::Command(
            RtmpCoreCommand::ClientHandleSetPeerBandwidth {
                size: 123_456,
                response_window_size: 5_000_000,
            },
        ))
        .expect("client handle set peer bandwidth");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::LocalAckWindowUpdated { size: 123_456 })
    )));
    assert!(out
        .iter()
        .any(|output| matches!(output, CoreOutput::Write(_))));
}

#[test]
fn client_observe_media_data_command_emits_media_data_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let out = core
        .handle_input(CoreInput::Command(
            RtmpCoreCommand::ClientObserveMediaData {
                stream_id: 9,
                timestamp_ms: 1234,
                media_type: crate::core::RtmpMediaType::Audio,
                payload: Bytes::from_static(&[0xAF, 0x01, 0x12, 0x34]),
            },
        ))
        .expect("client observe media data");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::MediaData {
            stream_id: 9,
            timestamp_ms: 1234,
            media_type: crate::core::RtmpMediaType::Audio,
            ..
        })
    )));
}

#[test]
fn client_handle_user_control_ping_request_emits_ping_response_write() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let out = core
        .handle_input(CoreInput::Command(
            RtmpCoreCommand::ClientHandleUserControl {
                event: RtmpUserControlEvent::PingRequest {
                    timestamp: RtmpTimestamp::from_millis(42),
                },
            },
        ))
        .expect("client handle user control");

    let mut decoder = RtmpChunkDecoder::default();
    let mut pending = Vec::new();
    let mut found_ping_response = false;
    for output in out {
        let CoreOutput::Write(bytes) = output else {
            continue;
        };
        pending.extend_from_slice(&bytes);
        loop {
            match decoder.decode(&pending) {
                Ok((consumed, maybe_chunk)) => {
                    pending.drain(..consumed);
                    let Some(chunk) = maybe_chunk else {
                        continue;
                    };
                    if chunk.message_type != RtmpMessageType::UserControl || chunk.payload.len() < 6
                    {
                        continue;
                    }
                    let event_type = u16::from_be_bytes([chunk.payload[0], chunk.payload[1]]);
                    let timestamp = u32::from_be_bytes([
                        chunk.payload[2],
                        chunk.payload[3],
                        chunk.payload[4],
                        chunk.payload[5],
                    ]);
                    if event_type == 7 && timestamp == 42 {
                        found_ping_response = true;
                    }
                }
                Err(err) if err.kind == ErrorKind::InsufficientBuffer => break,
                Err(err) => panic!("decode chunk failed: {err:?}"),
            }
        }
    }

    assert!(found_ping_response);
}

#[test]
fn client_handle_user_control_non_ping_emits_user_control_ignored_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let out = core
        .handle_input(CoreInput::Command(
            RtmpCoreCommand::ClientHandleUserControl {
                event: RtmpUserControlEvent::BufferReady {
                    stream_id: RtmpMessageStreamId::new(7),
                },
            },
        ))
        .expect("client handle user control");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::UserControlIgnored { name, detail })
            if name == "BufferReady" && detail.contains("BufferReady")
    )));
}

#[test]
fn client_handle_unhandled_message_emits_message_ignored_event() {
    let mut core = RtmpCore::new();
    core.state = HandshakeState::Ready;

    let out = core
        .handle_input(CoreInput::Command(
            RtmpCoreCommand::ClientHandleUnhandledMessage {
                message: RtmpMessage::Abort {
                    header: RtmpMessageHeader::PCM,
                    chunk_stream_id: RtmpChunkStreamId::new(3).expect("valid chunk stream id"),
                },
            },
        ))
        .expect("client handle unhandled message");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::MessageIgnored { name, detail })
            if name == "Abort" && detail.contains("chunk_stream_id")
    )));
}
