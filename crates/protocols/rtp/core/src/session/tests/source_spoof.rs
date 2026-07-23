use super::*;

fn ps_packet(ssrc: u32, seq: u16) -> RtpPacket {
    RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: seq,
            timestamp: 1000,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x00, 0x00, 0x00, 0x00]),
    }
}

fn ps_datagram_at(ssrc: u32, seq: u16, source: SocketAddr, received_at_ms: u64) -> RtpDatagram {
    RtpDatagram {
        source,
        data: ps_packet(ssrc, seq).encode(),
        received_at_ms,
    }
}

fn ps_datagram(ssrc: u32, seq: u16, source: SocketAddr) -> RtpDatagram {
    ps_datagram_at(ssrc, seq, source, 0)
}

fn server_spec(key: &str, ssrc: u32, source_policy: RtpSourcePolicy) -> RtpServerSpec {
    RtpServerSpec {
        session_key: key.to_string(),
        ssrc: Some(ssrc),
        payload_mode: RtpPayloadMode::RawAudio,
        transport_mode: RtpTransportMode::RecvOnly,
        packet_duration_ms: None,
        connection_type: None,
        source_policy: Some(source_policy),
        track_filter: RtpTrackFilter::All,
    }
}

#[test]
fn source_rebind_rate_limit_blocks_spoof_after_max() {
    let mut core = RtpCore::new(10, 5000);
    core.set_source_rebind_idle_window_ms(1);
    core.set_max_source_rebinds(2);

    let key = "rebind-limit".to_string();
    let ssrc = 1000u32;
    let a: SocketAddr = "127.0.0.1:1001".parse().unwrap();
    let b: SocketAddr = "127.0.0.1:1002".parse().unwrap();
    let c: SocketAddr = "127.0.0.1:1003".parse().unwrap();
    let d: SocketAddr = "127.0.0.1:1004".parse().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        server_spec(&key, ssrc, RtpSourcePolicy::AllowValidatedRebind),
    )));

    // First packet binds source A.
    let _ = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram_at(ssrc, 100, a, 0)));
    let session = core.sessions.get(&key).expect("session");
    assert_eq!(session.source_addr, Some(a));
    assert_eq!(session.source_rebind_count, 0);

    // Plausible rebind to B.
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram_at(ssrc, 101, b, 1)));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SourceChanged { session_key, .. }) if session_key == &key
    )));
    let session = core.sessions.get(&key).expect("session");
    assert_eq!(session.source_addr, Some(b));
    assert_eq!(session.source_rebind_count, 1);

    // Plausible rebind to C.
    let _ = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram_at(ssrc, 102, c, 2)));
    let session = core.sessions.get(&key).expect("session");
    assert_eq!(session.source_addr, Some(c));
    assert_eq!(session.source_rebind_count, 2);

    // D is now over the rate limit and should be treated as a spoof.
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram_at(ssrc, 103, d, 3)));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SourceSpoofed { ssrc: s, .. }) if *s == ssrc
    )));
    let session = core.sessions.get(&key).expect("session");
    assert_eq!(session.source_addr, Some(c));
    assert_eq!(session.source_rebind_count, 2);
}

#[test]
fn ssrc_continuity_prevents_cross_session_injection() {
    let mut core = RtpCore::new(10, 5000);

    let key_a = "session-a".to_string();
    let key_b = "session-b".to_string();
    let ssrc_a = 1000u32;
    let ssrc_b = 2000u32;
    let source_a: SocketAddr = "127.0.0.1:1001".parse().unwrap();
    let source_b: SocketAddr = "127.0.0.1:1002".parse().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        server_spec(&key_a, ssrc_a, RtpSourcePolicy::AllowValidatedRebind),
    )));
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        server_spec(&key_b, ssrc_b, RtpSourcePolicy::AllowValidatedRebind),
    )));

    // Session A receives from source A.
    let _ = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc_a, 100, source_a)));

    // An attacker using session B's source address but session A's SSRC is routed to session A
    // and rejected as a source spoof.
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc_a, 101, source_b)));
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Diagnostic(RtpCoreDiagnostic::SourceSpoofed { ssrc: s, .. }) if *s == ssrc_a
    )));
    let session = core.sessions.get(&key_a).expect("session");
    assert_eq!(session.source_addr, Some(source_a));
    assert_eq!(session.source_spoof_count, 1);

    // Session B is not affected.
    let _ = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc_b, 100, source_b)));
    let session_b = core.sessions.get(&key_b).expect("session");
    assert_eq!(session_b.source_addr, Some(source_b));
    assert_eq!(session_b.source_spoof_count, 0);
}
