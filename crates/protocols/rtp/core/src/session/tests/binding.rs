use super::*;

fn es_packet(ssrc: u32, seq: u16) -> RtpPacket {
    RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: seq,
            timestamp: u32::from(seq),
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x09]),
    }
}

fn ps_packet(ssrc: u32, seq: u16) -> RtpPacket {
    RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 97,
            sequence_number: seq,
            timestamp: u32::from(seq),
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA]),
    }
}

fn udp_dgram(source: &str, rtp: RtpPacket, received_at_ms: u64) -> RtpCoreInput {
    RtpCoreInput::UdpPacket(RtpDatagram {
        source: source.parse().unwrap(),
        data: rtp.encode(),
        received_at_ms,
    })
}

#[test]
fn test_strict_source_binding_drops_spoofed_packets() {
    let mut core = RtpCore::new(10, 30_000);
    let key = "recv/binding".to_string();
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Es,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let legit = udp_dgram("127.0.0.1:5000", es_packet(1000, 1), 0);
    let spoof = udp_dgram("127.0.0.1:6000", es_packet(1000, 2), 0);
    let legit2 = udp_dgram("127.0.0.1:5000", es_packet(1000, 3), 0);

    let _ = core.handle_input(legit);
    let spoof_outputs = core.handle_input(spoof);
    let _ = core.handle_input(legit2);

    let session = core.sessions.get(&key).expect("session alive");
    assert_eq!(session.source_addr, Some("127.0.0.1:5000".parse().unwrap()));
    assert_eq!(session.packets_received, 2);
    assert_eq!(session.source_spoof_count, 1);

    assert!(spoof_outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SourceSpoofed { ssrc: 1000, .. })
    )));
}

#[test]
fn test_allow_validated_rebind_after_idle_window() {
    let mut core = RtpCore::new(10, 30_000);
    core.set_source_rebind_idle_window_ms(100);

    let key = "recv/rebind".to_string();
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Es,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: key.clone(),
        expected_generation: 1,
        ssrc: None,
        payload_type: None,
        pause_check: None,
        source_policy: Some(RtpSourcePolicy::AllowValidatedRebind),
    }));

    let outputs = core.handle_input(udp_dgram("127.0.0.1:5000", es_packet(1000, 1), 0));
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::SourceChanged { .. }))));

    let outputs = core.handle_input(udp_dgram("127.0.0.1:6000", es_packet(1000, 2), 150));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SourceChanged {
            session_key,
            old,
            new,
        }) if session_key == &key && *old == "127.0.0.1:5000".parse().unwrap() && *new == "127.0.0.1:6000".parse().unwrap()
    )));

    let session = core.sessions.get(&key).expect("session alive");
    assert_eq!(session.source_addr, Some("127.0.0.1:6000".parse().unwrap()));
    assert_eq!(session.source_rebind_count, 1);
    assert_eq!(session.packets_received, 2);

    // A packet from the new source that is too soon (not idle) should still be rejected
    // because the inter-packet gap is only 10 ms.
    let outputs = core.handle_input(udp_dgram("127.0.0.1:7000", es_packet(1000, 3), 160));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SourceSpoofed { ssrc: 1000, .. })
    )));

    let session = core.sessions.get(&key).expect("session still alive");
    assert_eq!(session.source_addr, Some("127.0.0.1:6000".parse().unwrap()));
    assert_eq!(session.source_spoof_count, 1);
}

#[test]
fn test_rebind_rate_limit_blocks_excess_rebinds() {
    let mut core = RtpCore::new(10, 30_000);
    core.set_source_rebind_idle_window_ms(1);
    core.set_max_source_rebinds(1);

    let key = "recv/ratelimit".to_string();
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Es,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: key.clone(),
        expected_generation: 1,
        ssrc: None,
        payload_type: None,
        pause_check: None,
        source_policy: Some(RtpSourcePolicy::AllowValidatedRebind),
    }));

    let _ = core.handle_input(udp_dgram("127.0.0.1:5000", es_packet(1000, 1), 0));
    let _ = core.handle_input(udp_dgram("127.0.0.1:6000", es_packet(1000, 2), 10));
    let outputs = core.handle_input(udp_dgram("127.0.0.1:7000", es_packet(1000, 3), 20));

    let session = core.sessions.get(&key).expect("session alive");
    assert_eq!(session.source_addr, Some("127.0.0.1:6000".parse().unwrap()));
    assert_eq!(session.source_rebind_count, 1);
    assert_eq!(session.source_spoof_count, 1);

    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SourceSpoofed { ssrc: 1000, .. })
    )));
}

#[test]
fn test_strict_reject_spoof_before_pt_resolution() {
    let mut core = RtpCore::new(10, 30_000);
    let key = "recv/spoof-pt".to_string();
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(1000),
            payload_mode: RtpPayloadMode::Es,
            transport_mode: RtpTransportMode::RecvOnly,
            connection_type: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let _ = core.handle_input(udp_dgram("127.0.0.1:5000", es_packet(1000, 1), 0));
    let spoof_outputs = core.handle_input(udp_dgram("127.0.0.1:6000", ps_packet(1000, 2), 0));

    let session = core.sessions.get(&key).expect("session alive");
    assert_eq!(session.source_addr, Some("127.0.0.1:5000".parse().unwrap()));
    assert_eq!(session.payload_type, Some(96));
    assert_eq!(session.payload_mode, RtpPayloadMode::Es);
    assert_eq!(session.pt_format_change_count, 0);
    assert_eq!(session.packets_received, 1);
    assert_eq!(session.source_spoof_count, 1);

    assert!(!spoof_outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::Event(RtpCoreEvent::FormatChanged { .. }))));
    assert!(spoof_outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SourceSpoofed { ssrc: 1000, .. })
    )));
}
