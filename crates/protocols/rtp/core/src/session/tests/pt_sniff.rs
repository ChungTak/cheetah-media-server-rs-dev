use super::*;

#[test]
fn test_pt_resolver_sniffs_h26x_on_auto_create_after_confirmation() {
    let mut core = RtpCore::new(10, 30_000);

    // Two consecutive Annex-B packets are required before committing to Es.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 1000,
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
        .get("live/1000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Es);
}

#[test]
fn test_single_annexb_hit_does_not_commit_to_es() {
    let mut core = RtpCore::new(10, 30_000);

    // First packet looks like Annex-B but the second packet is a PS pack header,
    // so the stream should resolve to Ps, not Es.
    let first = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc: 1000,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: first.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let second = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 2,
            timestamp: 1,
            ssrc: 1000,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x00]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: second.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let session = core
        .sessions
        .get("live/1000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Ps);
}

#[test]
fn test_unknown_payload_falls_back_to_ps_after_probe_budget() {
    let mut core = RtpCore::new(10, 30_000);

    for seq in 1..=8u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 2000,
                marker: false,
            },
            payload: Bytes::from(vec![0xAB, 0xCD]),
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
        .get("live/2000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Ps);
}
