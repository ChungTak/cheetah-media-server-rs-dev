use super::*;

#[test]
fn test_pt_lock_confidence_requires_consecutive_matches() {
    let mut core = RtpCore::new(10, 30_000);
    core.set_pt_lock_confidence(3);

    // Two Annex-B packets are not enough to commit with confidence 3.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 3000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    }

    let session = core
        .sessions
        .get("live/3000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Unknown);

    // A non-matching packet resets the counter, so the next two Annex-B hits
    // still do not reach confidence 3.
    let mismatch = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 3,
            timestamp: 3,
            ssrc: 3000,
            marker: false,
        },
        payload: Bytes::from(vec![0xAB, 0xCD]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: mismatch.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    for seq in 4..=5u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 3000,
                marker: false,
            },
            payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    }

    let session = core
        .sessions
        .get("live/3000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Unknown);

    // The third consecutive Annex-B packet locks the mode to Es.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 6,
            timestamp: 6,
            ssrc: 3000,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let session = core
        .sessions
        .get("live/3000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Es);
}
