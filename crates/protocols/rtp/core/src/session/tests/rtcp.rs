use super::*;

#[test]
fn test_rtcp_bye_closes_matching_session() {
    let mut core = RtpCore::new(10, 30_000);
    let ssrc = 0x4444_4444u32;
    let key = format!("live/{ssrc}");

    // Auto-create a session by feeding an RTP packet.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x00]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(core.sessions.contains_key(&key));

    // The peer sends RTCP BYE for its SSRC.
    let bye = RtcpCompoundPacket {
        packets: vec![RtcpPacket::Bye(RtcpBye {
            ssrcs: vec![ssrc],
            reason: Some("shutdown".to_string()),
        })],
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:2".parse().unwrap(),
        data: bye.encode().unwrap(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::RtcpPacket(dgram));

    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::CloseSession(k) if k == &key)),
        "BYE must emit explicit CloseSession action"
    );
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { session_key, .. })
        if session_key == &key
    )));
    assert!(!core.sessions.contains_key(&key));
}

#[test]
fn test_rtcp_bye_ignores_unknown_ssrc() {
    let mut core = RtpCore::new(10, 30_000);
    let ssrc = 0x1111_1111u32;
    let key = format!("live/{ssrc}");

    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x00]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let bye = RtcpCompoundPacket {
        packets: vec![RtcpPacket::Bye(RtcpBye {
            ssrcs: vec![0x2222_2222],
            reason: None,
        })],
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:2".parse().unwrap(),
        data: bye.encode().unwrap(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::RtcpPacket(dgram));

    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::CloseSession(_))));
    assert!(core.sessions.contains_key(&key));
}
