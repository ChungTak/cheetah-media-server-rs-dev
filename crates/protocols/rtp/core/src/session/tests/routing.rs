use super::*;

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
        source_policy: None,
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
        source_policy: None,
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
