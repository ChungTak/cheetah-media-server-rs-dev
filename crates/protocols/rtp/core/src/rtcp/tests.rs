use super::*;
use bytes::Bytes;

#[test]
fn roundtrip_sender_report_with_report_block() {
    let sr = RtcpSenderReport {
        ssrc: 0x12345678,
        ntp_timestamp: 0x1234567890abcdef,
        rtp_timestamp: 0xdeadbeef,
        packets_sent: 1000,
        octets_sent: 50000,
        report_blocks: vec![RtcpReportBlock {
            ssrc: 0x87654321,
            fraction_lost: 10,
            cumulative_lost: -5,
            highest_seq: 0x0001_ffff,
            jitter: 1234,
            last_sr: 0,
            delay_since_last_sr: 0,
        }],
    };
    let encoded = sr.encode().unwrap();
    let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
    assert_eq!(decoded.packets.len(), 1);
    let RtcpPacket::SenderReport(parsed) = &decoded.packets[0] else {
        panic!("expected sender report");
    };
    assert_eq!(parsed.ssrc, sr.ssrc);
    assert_eq!(parsed.ntp_timestamp, sr.ntp_timestamp);
    assert_eq!(parsed.rtp_timestamp, sr.rtp_timestamp);
    assert_eq!(parsed.packets_sent, sr.packets_sent);
    assert_eq!(parsed.octets_sent, sr.octets_sent);
    assert_eq!(parsed.report_blocks, sr.report_blocks);
}

#[test]
fn roundtrip_receiver_report() {
    let rr = RtcpReceiverReport {
        ssrc: 0x11111111,
        report_blocks: vec![RtcpReportBlock {
            ssrc: 0x22222222,
            fraction_lost: 0,
            cumulative_lost: 0,
            highest_seq: 0x0000_1234,
            jitter: 0,
            last_sr: 0x5555_5555,
            delay_since_last_sr: 0x6666_6666,
        }],
    };
    let encoded = rr.encode().unwrap();
    let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
    assert_eq!(decoded.packets.len(), 1);
    let RtcpPacket::ReceiverReport(parsed) = &decoded.packets[0] else {
        panic!("expected receiver report");
    };
    assert_eq!(parsed.ssrc, rr.ssrc);
    assert_eq!(parsed.report_blocks, rr.report_blocks);
}

#[test]
fn roundtrip_source_description() {
    let sdes = RtcpSourceDescription {
        chunks: vec![RtcpSdesChunk {
            ssrc: 0x33333333,
            items: vec![RtcpSdesItem {
                item_type: RtcpSdesItemType::CName,
                text: "user@host".to_string(),
            }],
        }],
    };
    let encoded = sdes.encode().unwrap();
    let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
    assert_eq!(decoded.packets.len(), 1);
    let RtcpPacket::SourceDescription(parsed) = &decoded.packets[0] else {
        panic!("expected sdes");
    };
    assert_eq!(parsed.chunks.len(), 1);
    assert_eq!(parsed.chunks[0].ssrc, 0x33333333);
    assert_eq!(parsed.chunks[0].items.len(), 1);
    assert_eq!(parsed.chunks[0].items[0].item_type, RtcpSdesItemType::CName);
    assert_eq!(parsed.chunks[0].items[0].text, "user@host");
}

#[test]
fn roundtrip_source_description_with_padding() {
    // 10-byte text gives item length 12 and requires 3 trailing pad bytes.
    let sdes = RtcpSourceDescription {
        chunks: vec![RtcpSdesChunk {
            ssrc: 0x55555555,
            items: vec![RtcpSdesItem {
                item_type: RtcpSdesItemType::CName,
                text: "0123456789".to_string(),
            }],
        }],
    };
    let encoded = sdes.encode().unwrap();
    assert_eq!(encoded.len() % 4, 0);
    let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
    let RtcpPacket::SourceDescription(parsed) = &decoded.packets[0] else {
        panic!("expected sdes");
    };
    assert_eq!(parsed.chunks[0].items[0].text, "0123456789");
}

#[test]
fn roundtrip_bye_with_reason() {
    let bye = RtcpBye {
        ssrcs: vec![0x44444444],
        reason: Some("gone".to_string()),
    };
    let encoded = bye.encode().unwrap();
    let decoded = RtcpCompoundPacket::parse(encoded).unwrap();
    assert_eq!(decoded.packets.len(), 1);
    let RtcpPacket::Bye(parsed) = &decoded.packets[0] else {
        panic!("expected bye");
    };
    assert_eq!(parsed.ssrcs, bye.ssrcs);
    assert_eq!(parsed.reason, bye.reason);
}

#[test]
fn parses_compound_rr_plus_sdes_plus_bye() {
    let compound = RtcpCompoundPacket {
        packets: vec![
            RtcpPacket::ReceiverReport(RtcpReceiverReport {
                ssrc: 0x11111111,
                report_blocks: Vec::new(),
            }),
            RtcpPacket::SourceDescription(RtcpSourceDescription {
                chunks: vec![RtcpSdesChunk {
                    ssrc: 0x11111111,
                    items: vec![RtcpSdesItem {
                        item_type: RtcpSdesItemType::CName,
                        text: "c".to_string(),
                    }],
                }],
            }),
            RtcpPacket::Bye(RtcpBye {
                ssrcs: vec![0x11111111],
                reason: None,
            }),
        ],
    };
    let encoded = compound.encode().unwrap();
    let parsed = RtcpCompoundPacket::parse(encoded).unwrap();
    assert_eq!(parsed, compound);
}

#[test]
fn rejects_short_rtcp_packet() {
    // A minimal RR with no report blocks is 8 bytes: header + sender SSRC.
    assert!(RtcpCompoundPacket::parse(Bytes::from_static(&[
        0x80, 201, 0, 1, 0x11, 0x11, 0x11, 0x11
    ]))
    .is_ok());
    // A 2-byte packet cannot contain the common header.
    assert!(RtcpCompoundPacket::parse(Bytes::from_static(&[0x80, 201])).is_err());
}

#[test]
fn parses_padded_bye_with_reason() {
    // BYE with one SSRC, reason "x" (1 byte), and 2 padding bytes to reach 12 bytes total.
    // Header: V=2, P=1, count=1, PT=203, length=2 (8 bytes header + 4 bytes body).
    // Body: SSRC (4) + reason_len 1 + 'x' + pad_count 2 (2 bytes total -> body 6? Wait total 12 -> length=2 -> body 8)
    // Actually BYE with 1 ssrc (4) + reason_len (1) + text (1) + padding (2) = 8 body. Total 12, length=2.
    let raw = Bytes::from_static(&[
        0xa1, 203, 0, 2, 0x44, 0x44, 0x44, 0x44, 0x01, b'x', 0x00, 0x02,
    ]);
    let parsed = RtcpCompoundPacket::parse(raw).unwrap();
    let RtcpPacket::Bye(bye) = &parsed.packets[0] else {
        panic!("expected bye");
    };
    assert_eq!(bye.ssrcs, vec![0x44444444]);
    assert_eq!(bye.reason, Some("x".to_string()));
}

#[test]
fn rejects_invalid_padding() {
    // pad_count (9) exceeds body length (8).
    let raw = Bytes::from_static(&[
        0xa1, 203, 0, 2, 0x44, 0x44, 0x44, 0x44, 0x01, b'x', 0x00, 0x09,
    ]);
    assert!(RtcpCompoundPacket::parse(raw).is_err());
}

#[test]
fn rejects_overlong_sdes_text() {
    let text = "x".repeat(300);
    let sdes = RtcpSourceDescription {
        chunks: vec![RtcpSdesChunk {
            ssrc: 0x33333333,
            items: vec![RtcpSdesItem {
                item_type: RtcpSdesItemType::CName,
                text,
            }],
        }],
    };
    assert!(matches!(
        sdes.encode(),
        Err(RtcpEncodeError::SdesItemTooLong { .. })
    ));
}

#[test]
fn rejects_overlong_bye_reason() {
    let reason = "x".repeat(300);
    let bye = RtcpBye {
        ssrcs: vec![0x44444444],
        reason: Some(reason),
    };
    assert!(matches!(
        bye.encode(),
        Err(RtcpEncodeError::ByeReasonTooLong { .. })
    ));
}

#[test]
fn parses_unknown_rtcp_packet_without_aborting() {
    // Construct a minimal unknown PT=205 (RTPFB) packet with 4 bytes payload.
    // Header: V=2, count=0, PT=205, length=1 (8 bytes total).
    let raw = Bytes::from_static(&[0x80, 205, 0, 1, 0xde, 0xad, 0xbe, 0xef]);
    let parsed = RtcpCompoundPacket::parse(raw).unwrap();
    assert_eq!(parsed.packets.len(), 1);
    let RtcpPacket::Unknown { pt, payload, .. } = &parsed.packets[0] else {
        panic!("expected unknown");
    };
    assert_eq!(*pt, 205);
    assert_eq!(payload.as_ref(), &[0xde, 0xad, 0xbe, 0xef]);
}

#[test]
fn parses_unknown_rtcp_app_packet() {
    // App packet: header, ssrc=0x11111111, name="test", no payload (12 bytes total).
    let raw = Bytes::from_static(&[
        0x80, 204, 0, 2, 0x11, 0x11, 0x11, 0x11, b't', b'e', b's', b't',
    ]);
    let parsed = RtcpCompoundPacket::parse(raw).unwrap();
    let RtcpPacket::App(app) = &parsed.packets[0] else {
        panic!("expected app");
    };
    assert_eq!(app.ssrc, 0x11111111);
    assert_eq!(app.name, [b't', b'e', b's', b't']);
    assert!(app.payload.is_empty());
}

#[test]
fn compound_with_unknown_packet_roundtrips() {
    let compound = RtcpCompoundPacket {
        packets: vec![
            RtcpPacket::ReceiverReport(RtcpReceiverReport {
                ssrc: 0x11111111,
                report_blocks: Vec::new(),
            }),
            RtcpPacket::Unknown {
                pt: 205,
                count: 0,
                payload: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            },
            RtcpPacket::Bye(RtcpBye {
                ssrcs: vec![0x11111111],
                reason: None,
            }),
        ],
    };
    let encoded = compound.encode().unwrap();
    let parsed = RtcpCompoundPacket::parse(encoded).unwrap();
    assert_eq!(parsed.packets.len(), 3);
    assert!(matches!(
        parsed.packets[1],
        RtcpPacket::Unknown { pt: 205, .. }
    ));
}
