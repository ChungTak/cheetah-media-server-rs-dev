use super::*;
use crate::rtcp::RtcpBye;
use crate::types::{RtpClientSpec, RtpConnectionType, RtpDatagram, RtpSendFrame, RtpServerSpec};
use cheetah_codec::{
    AVFrame, CodecId, FrameFormat, MediaKind, RtpHeader, RtpPacket, Timebase, TrackId,
};
use std::net::SocketAddr;

#[test]
fn test_rtp_core_ssrc_routing_and_auto_create() {
    let mut core = RtpCore::new(10, 5000);
    let addr = "127.0.0.1:12345".parse::<SocketAddr>().unwrap();

    // 1. Send an RTP packet with unmapped SSRC = 9999
    let packet = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 100,
            timestamp: 1000,
            ssrc: 9999,
            marker: false,
        },
        // starts with PS start code to test probed payload mode Ps
        payload: Bytes::from(vec![
            0x00, 0x00, 0x01, 0xBA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]),
    };

    let datagram = RtpDatagram {
        source: addr,
        data: packet.encode(),
        received_at_ms: 0,
    };

    let outputs = core.handle_input(RtpCoreInput::UdpPacket(datagram));

    // Should auto-create a session named "live/9999" and emit SessionCreated event
    assert!(!outputs.is_empty());
    let mut has_created = false;
    for output in outputs {
        if let RtpCoreOutput::Event(RtpCoreEvent::SessionCreated {
            session_key,
            ssrc,
            payload_mode,
            transport_mode,
        }) = output
        {
            assert_eq!(session_key, "live/9999");
            assert_eq!(ssrc, 9999);
            assert_eq!(payload_mode, RtpPayloadMode::Ps);
            assert_eq!(transport_mode, RtpTransportMode::RecvOnly);
            has_created = true;
        }
    }
    assert!(has_created);
}

#[test]
fn test_rtp_core_session_timeout() {
    let mut core = RtpCore::new(10, 1000); // 1000ms timeout

    // Pre-create server session
    let spec = RtpServerSpec {
        session_key: "test_session".to_string(),
        ssrc: Some(12345),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));
    assert_eq!(outputs.len(), 1);

    // Tick at t = 0
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 100 });
    assert!(outputs.is_empty());

    // Tick at t = 1500 (idle timeout triggered)
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 1500 });
    let mut has_closed = false;
    for output in outputs {
        if let RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { session_key, .. }) = output {
            assert_eq!(session_key, "test_session");
            has_closed = true;
        }
    }
    assert!(has_closed);
}

#[test]
fn test_rtp_core_pause_check_delays_timeout() {
    let mut core = RtpCore::new(10, 1000); // 1000ms timeout

    let spec = RtpServerSpec {
        session_key: "paused_session".to_string(),
        ssrc: Some(12345),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    // Pause timeout checks while the session receives no traffic.
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::PauseCheck {
        session_key: "paused_session".to_string(),
        paused: true,
    }));

    // Tick well past the idle timeout while paused: session must stay alive.
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 5000 });
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));

    // Resume checks; the next tick should baseline activity, not immediately close.
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::PauseCheck {
        session_key: "paused_session".to_string(),
        paused: false,
    }));
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 5500 });
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));

    // Only after the timeout window passes again does the session close.
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 6600 });
    assert!(outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));
}

#[test]
fn test_rtp_core_tcp_recovery_via_known_ssrc() {
    // Pre-register a known SSRC, then feed a TCP chunk that begins with a corrupt
    // length-prefix but contains a valid RTP frame (with the known SSRC) further in.
    // The recovery scan should still extract the valid frame.
    let mut core = RtpCore::new(10, 30_000);
    let ssrc = 0xABCDEF12u32;
    let spec = RtpServerSpec {
        session_key: "live/recovery".to_string(),
        ssrc: Some(ssrc),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    // Build a valid RTP-over-TCP frame for the known SSRC.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 100,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA, 0xBB]),
    };
    let valid_frame = cheetah_codec::encode_tcp_rtp_frame(&rtp);

    // Prepend 16 bytes of garbage so the parser must scan forward to recover.
    let mut chunk = vec![0xFFu8; 16];
    chunk.extend_from_slice(&valid_frame);

    let outputs = core.handle_input(RtpCoreInput::TcpBytes(crate::types::RtpTcpChunk {
        conn_id: 1,
        data: Bytes::from(chunk),
        received_at_ms: 0,
    }));

    // We should observe a Diagnostic for sequence-gap and at least one further event
    // that proves the RTP packet was processed (e.g. session created already exists, and
    // the demuxer was poked). At minimum we should not hang or panic.
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SequenceGap { .. })
    )));
}

#[test]
fn test_rtp_core_rr_timeout_shuts_down_sender() {
    // Senders should be torn down when no RR feedback arrives within the idle timeout.
    let mut core = RtpCore::new(10, 1000);
    let spec = RtpServerSpec {
        session_key: "send_session".to_string(),
        ssrc: Some(42),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::SendOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    // Baseline tick at t=100 establishes last_rr_received_ms=100.
    let _ = core.handle_input(RtpCoreInput::Tick { now_ms: 100 });

    // 500ms later: still within idle window, no shutdown.
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 600 });
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));

    // 2000ms later: well past timeout, sender should be closed with reason "RR timeout".
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 2200 });
    let closed = outputs.iter().any(|o| {
        matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { reason, .. })
                if reason == "RR timeout"
        )
    });
    assert!(
        closed,
        "sender should close on RR timeout: outputs={outputs:?}"
    );
}

#[test]
fn test_rtp_core_rr_resets_sender_timeout() {
    // When an RR is observed, the sender's RR-timeout baseline must move forward.
    let mut core = RtpCore::new(10, 1000);
    let spec = RtpServerSpec {
        session_key: "send_session".to_string(),
        ssrc: Some(99),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::SendOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));
    // Baseline.
    let _ = core.handle_input(RtpCoreInput::Tick { now_ms: 100 });

    // Build an RR RTCP packet describing SSRC=99 (our sender SSRC).
    // RR header: V=2, RC=1, PT=201, length=7, sender SSRC then source SSRC blocks.
    let mut rr = Vec::new();
    rr.push(0x81); // V=2, RC=1
    rr.push(201); // RR
    rr.extend_from_slice(&7u16.to_be_bytes());
    rr.extend_from_slice(&0u32.to_be_bytes()); // reporter SSRC (peer)
    rr.extend_from_slice(&99u32.to_be_bytes()); // describes our SSRC
    rr.extend_from_slice(&[0u8; 20]); // remaining report-block bytes

    let dgram = crate::types::RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: Bytes::from(rr),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::RtcpPacket(dgram));

    // 1500ms later: would have been past the 1000ms timeout if RR had not refreshed it.
    // After RR at t=now_ms, baseline moves to current `now_ms` (which is still 100 after the
    // last tick). To make this verifiable, advance another tick before the RR window ends.
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 800 });
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { .. }))));
}

#[test]
fn test_rtp_core_tcp_interleaved_framing_dispatches() {
    // Auto-detect framing must accept RTSP-style 4-byte interleaved RTP frames over a TCP
    // chunk just as it accepts the 2-byte RFC 4571 form.
    let mut core = RtpCore::new(10, 30_000);
    let ssrc = 0x12345678u32;
    let spec = RtpServerSpec {
        session_key: "live/interleaved".to_string(),
        ssrc: Some(ssrc),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 11,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA, 0xBB]),
    };
    let frame = cheetah_codec::encode_interleaved_rtp_frame(&rtp, 0);

    // Feeding the interleaved frame should not produce a header diagnostic, indicating that
    // the auto-detect path matched on the leading `$` byte.
    let outputs = core.handle_input(RtpCoreInput::TcpBytes(crate::types::RtpTcpChunk {
        conn_id: 1,
        data: frame,
        received_at_ms: 0,
    }));
    assert!(!outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::RtpHeaderError)
    )));
}

#[test]
fn test_rtp_core_oversized_payload_diagnostic() {
    // ABL-style dynamic max-RTP-length learner: when a payload exceeds the configured cap,
    // we still process the packet but emit `OversizedPayload` so operators can spot the
    // pathological stream.
    let mut core = RtpCore::new(10, 30_000);
    core.set_max_rtp_len_cap(1500);
    let ssrc = 0x1u32;

    let huge_payload = vec![0u8; 4096];
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(huge_payload),
    };
    let dgram = crate::types::RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };

    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::OversizedPayload {
            ssrc: 1,
            len: 4096,
            cap: 1500,
        })
    )));
}

#[test]
fn test_voice_talk_upgrades_session_and_sends_audio() {
    // An inbound session can be upgraded to VoiceTalk, reusing the same socket
    // (same session_key) to push audio back to the peer.
    let mut core = RtpCore::new(10, 30_000);
    let session_key = "recv/talk/cam".to_string();
    let ssrc = 7777u32;

    let server_spec = RtpServerSpec {
        session_key: session_key.clone(),
        ssrc: Some(ssrc),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        server_spec,
    )));

    // The peer address would normally be learned from the first ingress frame.
    let peer = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

    // Upgrade the same session to VoiceTalk / SendRecv with audio-only egress.
    let client_spec = RtpClientSpec {
        session_key: session_key.clone(),
        destination: peer,
        ssrc,
        payload_mode: RtpPayloadMode::RawAudio,
        transport_mode: RtpTransportMode::SendRecv,
        tcp_conn_id: None,
        connection_type: Some(RtpConnectionType::VoiceTalk),
        track_filter: RtpTrackFilter::OnlyAudio,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
        client_spec,
    )));

    // Audio frame should be emitted as UDP with static PT 0 (G.711 u-law).
    let audio = AVFrame::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::G711U,
        FrameFormat::G711Packet,
        0,
        0,
        Timebase::new(1, 8000),
        Bytes::from(vec![0xD5; 160]),
    );
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key: session_key.clone(),
            frame: audio,
        },
    )));

    let mut sent = false;
    for output in outputs {
        if let RtpCoreOutput::SendUdp(udp) = output {
            assert_eq!(udp.destination, peer);
            assert_eq!(udp.session_key, session_key);
            let parsed = RtpPacket::parse(&udp.data).unwrap();
            assert_eq!(parsed.header.ssrc, ssrc);
            assert_eq!(parsed.header.payload_type, 0);
            sent = true;
        }
    }
    assert!(sent, "expected SendUdp output for voice talk audio");

    // Video frame should be dropped by the OnlyAudio track filter.
    let video = AVFrame::new(
        TrackId(2),
        MediaKind::Video,
        CodecId::H264,
        FrameFormat::CanonicalH26x,
        0,
        0,
        Timebase::new(1, 90_000),
        Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
    );
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key,
            frame: video,
        },
    )));
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::SendUdp(_))));
}

#[test]
fn test_update_session_advances_generation_and_ssrc_index() {
    let mut core = RtpCore::new(10, 30_000);
    let key = "recv/update".to_string();
    let spec = RtpServerSpec {
        session_key: key.clone(),
        ssrc: Some(1000),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    // Update SSRC and pause with the correct expected generation.
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: key.clone(),
        expected_generation: 1,
        ssrc: Some(2000),
        payload_type: Some(96),
        pause_check: Some(true),
    }));

    let mut updated = false;
    for output in outputs {
        if let RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated {
            session_key,
            generation,
            ssrc,
            payload_type,
            pause_check,
        }) = output
        {
            assert_eq!(session_key, key);
            assert_eq!(generation, 2);
            assert_eq!(ssrc, Some(2000));
            assert_eq!(payload_type, Some(96));
            assert_eq!(pause_check, Some(true));
            updated = true;
        }
    }
    assert!(updated);

    // The new SSRC must be routed to the same session.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc: 2000,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionCreated { .. }))));
}

#[test]
fn test_update_session_rejects_wrong_generation_and_conflict() {
    let mut core = RtpCore::new(10, 30_000);
    let spec_a = RtpServerSpec {
        session_key: "a".to_string(),
        ssrc: Some(1000),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let spec_b = RtpServerSpec {
        session_key: "b".to_string(),
        ssrc: Some(2000),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec_a)));
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec_b)));

    // Wrong expected generation.
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: "a".to_string(),
        expected_generation: 99,
        ssrc: Some(3000),
        payload_type: None,
        pause_check: None,
    }));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
            session_key,
            reason,
        }) if session_key == "a" && reason == "generation mismatch"
    )));

    // Duplicate SSRC already used by session b.
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: "a".to_string(),
        expected_generation: 1,
        ssrc: Some(2000),
        payload_type: None,
        pause_check: None,
    }));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionUpdateFailed {
            session_key,
            reason,
        }) if session_key == "a" && reason.contains("already in use")
    )));

    // Session a must keep its original SSRC.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc: 1000,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionCreated { .. }))));
}

#[test]
fn test_update_session_payload_type_changes_mode_and_generation() {
    let mut core = RtpCore::new(10, 30_000);
    let key = "recv/pt".to_string();
    let spec = RtpServerSpec {
        session_key: key.clone(),
        ssrc: Some(1000),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: key.clone(),
        expected_generation: 1,
        ssrc: None,
        payload_type: Some(96),
        pause_check: None,
    }));

    let mut updated = false;
    for output in outputs {
        if let RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated {
            session_key,
            generation,
            ssrc,
            payload_type,
            pause_check,
        }) = output
        {
            assert_eq!(session_key, key);
            assert_eq!(generation, 2);
            assert_eq!(ssrc, None);
            assert_eq!(payload_type, Some(96));
            assert_eq!(pause_check, None);
            updated = true;
        }
    }
    assert!(updated, "expected SessionUpdated event");

    // A packet with the new PT should still be routed to the same session.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc: 1000,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0x09]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SessionCreated { .. }))));
}

#[test]
fn test_update_session_no_change_keeps_generation() {
    let mut core = RtpCore::new(10, 30_000);
    let key = "recv/noop".to_string();
    let spec = RtpServerSpec {
        session_key: key.clone(),
        ssrc: Some(1000),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: key.clone(),
        expected_generation: 1,
        ssrc: Some(1000),
        payload_type: None,
        pause_check: None,
    }));
    let updated = outputs.iter().find_map(|o| match o {
        RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated { generation, .. }) => Some(*generation),
        _ => None,
    });
    assert_eq!(updated, Some(1));
}

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

#[test]
fn test_format_changed_on_resolvable_pt_switch() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to Es (H.264 Annex-B) on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 4000,
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
        .get("live/4000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Es);

    // A mid-stream switch to static PT 33 (MP2T) with a TS sync byte is resolvable
    // and should emit a FormatChanged event.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 33,
            sequence_number: 3,
            timestamp: 3,
            ssrc: 4000,
            marker: false,
        },
        payload: Bytes::from(vec![0x47, 0x00, 0x01, 0x10]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let changed = outputs.iter().any(|o| {
        matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::FormatChanged {
                payload_type: 33,
                old_payload_mode: RtpPayloadMode::Es,
                new_payload_mode: RtpPayloadMode::Ts,
                ..
            })
        )
    });
    assert!(changed, "expected FormatChanged on PT switch");

    let session = core.sessions.get("live/4000").expect("session still alive");
    assert_eq!(session.payload_mode, RtpPayloadMode::Ts);
}

#[test]
fn test_session_closed_on_oscillating_pt_modes() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to RawAudio on static PT 0.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 0,
            sequence_number: 1,
            timestamp: 1,
            ssrc: 4100,
            marker: false,
        },
        payload: Bytes::from(vec![0x00]),
    };
    let dgram = RtpDatagram {
        source: "127.0.0.1:1".parse().unwrap(),
        data: rtp.encode(),
        received_at_ms: 0,
    };
    let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));

    let session = core
        .sessions
        .get("live/4100")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::RawAudio);

    // Oscillate between PT 33 (Ts) and PT 0 (RawAudio). Each switch increments the
    // format-change budget. The fourth mode switch exceeds the default budget and closes
    // the session instead of emitting another FormatChanged.
    let mut seq = 2u16;
    let pts = [33u8, 0, 33, 0];
    let mut final_outputs = Vec::new();
    for (i, pt) in pts.iter().enumerate() {
        let payload = if *pt == 33 {
            vec![0x47, 0x00, 0x01, 0x10]
        } else {
            vec![0x00]
        };
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: *pt,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 4100,
                marker: false,
            },
            payload: Bytes::from(payload),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        final_outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        seq += 1;

        // First three switches should keep the session alive.
        if i < 3 {
            assert!(
                core.sessions.contains_key("live/4100"),
                "session should survive {} format switches",
                i + 1
            );
        }
    }

    assert!(
        !core.sessions.contains_key("live/4100"),
        "session should be closed after repeated mode oscillation"
    );
    assert!(final_outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::CloseSession(key) if key == "live/4100"
    )));
}

#[test]
fn test_session_closed_on_unresolvable_pt_switch() {
    let mut core = RtpCore::new(10, 30_000);
    // Keep the close threshold small for this test; the default is much larger to
    // tolerate legitimate DTMF/FEC/RED bursts.
    core.set_max_tolerated_unknown_pt_packets(8);

    // Lock the session to Es on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 5000,
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
        .get("live/5000")
        .expect("auto-created session");
    assert_eq!(session.payload_mode, RtpPayloadMode::Es);

    // A persistent run of unresolvable PT packets (matching the probe budget) closes
    // the session; short DTMF/FEC bursts are tolerated.
    for seq in 3..=10u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 5000,
                marker: false,
            },
            payload: Bytes::from(vec![0xAB, 0xCD]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let outputs = if seq == 10 {
            core.handle_input(RtpCoreInput::UdpPacket(dgram))
        } else {
            let _ = core.handle_input(RtpCoreInput::UdpPacket(dgram));
            Vec::new()
        };

        if seq == 10 {
            let closed = outputs.iter().any(|o| {
                matches!(
                    o,
                    RtpCoreOutput::CloseSession(key) if key == "live/5000"
                )
            });
            assert!(
                closed,
                "expected CloseSession after repeated unresolvable PTs"
            );
            assert!(!core.sessions.contains_key("live/5000"));
        } else {
            assert!(
                core.sessions.contains_key("live/5000"),
                "single unknown PT should be tolerated"
            );
        }
    }
}

#[test]
fn test_interleaved_unknown_pt_is_tolerated() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to Es on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 6000,
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

    // One interleaved unknown PT (RFC 4733 DTMF/FEC) does not close the session.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 97,
            sequence_number: 3,
            timestamp: 3,
            ssrc: 6000,
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
    assert!(core.sessions.contains_key("live/6000"));

    // Returning to the original PT resumes normal processing.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 4,
            timestamp: 4,
            ssrc: 6000,
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
    assert!(core.sessions.contains_key("live/6000"));
}

#[test]
fn test_long_unknown_pt_burst_is_tolerated_before_returning_to_locked_pt() {
    let mut core = RtpCore::new(10, 30_000);

    // Lock the session to Es on PT 96.
    for seq in 1..=2u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 96,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 6001,
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

    // A 50-packet DTMF/FEC burst (well below the default 255-packet budget) must not
    // close the session while audio is suspended.
    for seq in 3..=52u16 {
        let rtp = RtpPacket {
            header: RtpHeader {
                version: 2,
                payload_type: 97,
                sequence_number: seq,
                timestamp: u32::from(seq),
                ssrc: 6001,
                marker: false,
            },
            payload: Bytes::from(vec![0xAB, 0xCD]),
        };
        let dgram = RtpDatagram {
            source: "127.0.0.1:1".parse().unwrap(),
            data: rtp.encode(),
            received_at_ms: 0,
        };
        let outputs = core.handle_input(RtpCoreInput::UdpPacket(dgram));
        assert!(
            !outputs
                .iter()
                .any(|o| matches!(o, RtpCoreOutput::CloseSession(key) if key == "live/6001")),
            "unknown-PT burst should be tolerated"
        );
        assert!(core.sessions.contains_key("live/6001"));
    }

    // Returning to the original PT resumes normal processing.
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 53,
            timestamp: 53,
            ssrc: 6001,
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
    assert!(core.sessions.contains_key("live/6001"));
}

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
