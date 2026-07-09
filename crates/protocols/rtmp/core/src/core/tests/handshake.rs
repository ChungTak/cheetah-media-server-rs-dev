use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use bytes::Bytes;

use crate::amf0::{encode_all, Amf0Value};
use crate::chunk::RtmpChunkEncoder;
use crate::chunk::{RtmpChunk, RtmpChunkSize, RtmpChunkStreamId};
use crate::message::{RtmpMessageStreamId, RtmpMessageType};
use crate::timestamp::RtmpTimestamp;

use super::super::{CoreInput, CoreOutput, HandshakeState, RtmpCore, RtmpCoreError, RtmpEvent};

fn command_wire(message_stream_id: u32, payload: &[u8]) -> Bytes {
    let chunk = RtmpChunk {
        chunk_stream_id: RtmpChunkStreamId::new(3).expect("valid csid"),
        message_stream_id: RtmpMessageStreamId::new(message_stream_id),
        message_type: RtmpMessageType::CommandAmf0,
        timestamp: RtmpTimestamp::from_millis(0),
        payload: Bytes::from(payload.to_vec()),
    };
    let mut encoder = RtmpChunkEncoder::default();
    encoder.set_chunk_size(RtmpChunkSize::saturating_new(128));
    let mut wire = Vec::new();
    encoder.encode(&mut wire, &chunk);
    Bytes::from(wire)
}

#[test]
fn handshake_responds_with_s0s1s2() {
    let mut core = RtmpCore::new();
    let mut c0c1 = vec![3u8; 1537];
    c0c1[0] = 3;
    let out = core
        .handle_input(CoreInput::Bytes(Bytes::from(c0c1)))
        .expect("input");
    assert!(out.iter().any(|v| matches!(v, CoreOutput::Write(_))));
}

#[test]
fn handshake_s1_lenient_profile_has_zero_prefix_and_varies_with_c1() {
    let mut core = RtmpCore::new();
    let mut c0c1_a = vec![0u8; 1537];
    c0c1_a[0] = 3;
    for (idx, byte) in c0c1_a[1..].iter_mut().enumerate() {
        *byte = (idx as u8).wrapping_mul(3);
    }
    let out_a = core
        .handle_input(CoreInput::Bytes(Bytes::from(c0c1_a.clone())))
        .expect("handshake a");
    let wire_a = out_a
        .iter()
        .find_map(|output| match output {
            CoreOutput::Write(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .expect("s0s1s2 a");
    let s1_a = &wire_a[1..1537];
    assert_eq!(&s1_a[..8], &[0u8; 8]);

    let mut core_b = RtmpCore::new();
    let mut c0c1_b = c0c1_a;
    c0c1_b[20] ^= 0x5a;
    let out_b = core_b
        .handle_input(CoreInput::Bytes(Bytes::from(c0c1_b)))
        .expect("handshake b");
    let wire_b = out_b
        .iter()
        .find_map(|output| match output {
            CoreOutput::Write(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .expect("s0s1s2 b");
    let s1_b = &wire_b[1..1537];
    assert_ne!(s1_a, s1_b);
}

#[test]
fn c2_with_pipelined_connect_keeps_post_handshake_bytes() {
    let mut core = RtmpCore::new();
    let mut c0c1 = vec![0u8; 1537];
    c0c1[0] = 3;
    let _ = core
        .handle_input(CoreInput::Bytes(Bytes::from(c0c1)))
        .expect("c0c1");

    let connect_payload = encode_all(&[
        Amf0Value::String("connect".to_string()),
        Amf0Value::Number(1.0),
        Amf0Value::object([("app", Amf0Value::String("live".to_string()))]),
    ]);
    let connect_wire = command_wire(0, &connect_payload);

    let mut c2_and_connect = vec![0u8; 1536];
    c2_and_connect.extend_from_slice(&connect_wire);
    let out = core
        .handle_input(CoreInput::Bytes(Bytes::from(c2_and_connect)))
        .expect("c2+connect");

    assert!(out.iter().any(|output| matches!(
        output,
        CoreOutput::Event(RtmpEvent::Connected { app, .. }) if app == "live"
    )));
}

#[test]
fn invalid_handshake_version_closes_core_and_returns_error() {
    let mut core = RtmpCore::new();
    let mut c0c1 = vec![0u8; 1537];
    c0c1[0] = 2;
    let err = core
        .handle_input(CoreInput::Bytes(Bytes::from(c0c1)))
        .expect_err("invalid version must error");
    assert!(matches!(err, RtmpCoreError::InvalidHandshakeVersion(2)));
    assert_eq!(core.state, HandshakeState::Closed);
}
