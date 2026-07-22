use super::*;

fn rtp_packet(ssrc: u32, seq: u16, pt: u8, payload: &[u8]) -> RtpPacket {
    RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: pt,
            sequence_number: seq,
            timestamp: u32::from(seq),
            ssrc,
            marker: false,
        },
        payload: Bytes::copy_from_slice(payload),
    }
}

fn udp_dgram(source: &str, rtp: RtpPacket, received_at_ms: u64) -> RtpCoreInput {
    RtpCoreInput::UdpPacket(RtpDatagram {
        source: source.parse().unwrap(),
        data: rtp.encode(),
        received_at_ms,
    })
}

fn server(key: &str, ssrc: u32, mode: RtpPayloadMode, transport: RtpTransportMode) -> RtpCoreInput {
    RtpCoreInput::Command(RtpCoreCommand::CreateServer(RtpServerSpec {
        session_key: key.to_string(),
        ssrc: Some(ssrc),
        payload_mode: mode,
        transport_mode: transport,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    }))
}

fn assert_terminal(outputs: &[RtpCoreOutput], key: &str, expected: RtpSessionCloseReason) {
    let closed = outputs.iter().any(|o| {
        matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::SessionClosed {
                session_key,
                reason,
            }) if session_key == key && *reason == expected
        )
    });
    assert!(
        closed,
        "expected SessionClosed for {key} with {expected:?}: {outputs:?}"
    );

    let close = outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::CloseSession(k) if k == key));
    assert!(close, "expected CloseSession for {key}: {outputs:?}");

    let no_state_change = outputs.iter().any(|o| {
        matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::SessionStateChanged {
                new_state: RtpSessionState::Closed,
                ..
            })
        )
    });
    assert!(
        !no_state_change,
        "terminal state must be signaled only through SessionClosed, not SessionStateChanged"
    );
}

#[test]
fn test_terminal_event_matrix() {
    let mut core = RtpCore::new(10, 500);

    // 1. Explicit stop.
    let k1 = "term/stop".to_string();
    let _ = core.handle_input(server(
        &k1,
        1000,
        RtpPayloadMode::Ps,
        RtpTransportMode::RecvOnly,
    ));
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::StopSession(
        k1.clone(),
    )));
    assert_terminal(&outputs, &k1, RtpSessionCloseReason::Stopped);

    // 2. Idle timeout on a receiver with no traffic after the baseline tick.
    let k2 = "term/idle".to_string();
    let _ = core.handle_input(server(
        &k2,
        2000,
        RtpPayloadMode::Ps,
        RtpTransportMode::RecvOnly,
    ));
    let _ = core.handle_input(RtpCoreInput::Tick { now_ms: 100 });
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 700 });
    assert_terminal(&outputs, &k2, RtpSessionCloseReason::IdleTimeout);

    // 3. RR timeout on a pure sender.
    let k3 = "term/rr".to_string();
    let _ = core.handle_input(server(
        &k3,
        3000,
        RtpPayloadMode::Ps,
        RtpTransportMode::SendOnly,
    ));
    let _ = core.handle_input(RtpCoreInput::Tick { now_ms: 1000 });
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 2000 });
    assert_terminal(&outputs, &k3, RtpSessionCloseReason::RrTimeout);

    // 4. RTCP BYE.
    let k4 = "term/bye".to_string();
    let ssrc4 = 4000u32;
    let _ = core.handle_input(server(
        &k4,
        ssrc4,
        RtpPayloadMode::Ps,
        RtpTransportMode::RecvOnly,
    ));
    let _ = core.handle_input(udp_dgram(
        "127.0.0.1:5000",
        rtp_packet(ssrc4, 1, 33, &[0x47, 0x00, 0x01, 0x10]),
        0,
    ));
    let bye = RtcpCompoundPacket {
        packets: vec![RtcpPacket::Bye(RtcpBye {
            ssrcs: vec![ssrc4],
            reason: Some("shutdown".to_string()),
        })],
    };
    let outputs = core.handle_input(RtpCoreInput::RtcpPacket(RtpDatagram {
        source: "127.0.0.1:5001".parse().unwrap(),
        data: bye.encode().unwrap(),
        received_at_ms: 0,
    }));
    assert_terminal(&outputs, &k4, RtpSessionCloseReason::Bye);

    // 5. Payload-mode oscillation triggers SessionClosed (format change is terminal here).
    let k5 = "term/osc".to_string();
    let _ = core.handle_input(server(
        &k5,
        5000,
        RtpPayloadMode::Es,
        RtpTransportMode::RecvOnly,
    ));
    // Lock to ES via static PT 34 (H263/ES) with Annex-B start code.
    let _ = core.handle_input(udp_dgram(
        "127.0.0.1:5000",
        rtp_packet(5000, 1, 34, &[0x00, 0x00, 0x00, 0x01, 0x09]),
        0,
    ));
    // Lower the switch budget so a single Es->Ts->Es oscillation closes the session.
    core.max_pt_format_changes = 1;
    let _ = core.handle_input(udp_dgram(
        "127.0.0.1:5000",
        rtp_packet(5000, 2, 33, &[0x47, 0x00, 0x01, 0x10]),
        0,
    ));
    let outputs = core.handle_input(udp_dgram(
        "127.0.0.1:5000",
        rtp_packet(5000, 3, 34, &[0x00, 0x00, 0x00, 0x01, 0x09]),
        0,
    ));
    assert_terminal(
        &outputs,
        &k5,
        RtpSessionCloseReason::PayloadModeOscillation {
            from: RtpPayloadMode::Ts,
            to: RtpPayloadMode::Es,
        },
    );

    // 6. Unresolvable payload type switch.
    let k6 = "term/pt".to_string();
    let _ = core.handle_input(server(
        &k6,
        6000,
        RtpPayloadMode::Es,
        RtpTransportMode::RecvOnly,
    ));
    let _ = core.handle_input(udp_dgram(
        "127.0.0.1:5000",
        rtp_packet(6000, 1, 96, &[0x00, 0x00, 0x00, 0x01, 0x09]),
        0,
    ));
    let _ = core.handle_input(udp_dgram(
        "127.0.0.1:5000",
        rtp_packet(6000, 2, 96, &[0x00, 0x00, 0x00, 0x01, 0x09]),
        0,
    ));
    // PT 99 is unknown/statically none and payload has no recognizable signature, so the
    // unresolved-PT counter grows until the session is closed.
    let mut outputs = Vec::new();
    for seq in 3..260u16 {
        outputs = core.handle_input(udp_dgram(
            "127.0.0.1:5000",
            rtp_packet(6000, seq, 99, &[0xDE, 0xAD, 0xBE, 0xEF]),
            0,
        ));
        if outputs
            .iter()
            .any(|o| matches!(o, RtpCoreOutput::CloseSession(k) if k == &k6))
        {
            break;
        }
    }
    assert_terminal(
        &outputs,
        &k6,
        RtpSessionCloseReason::UnresolvablePayloadType {
            current: 96,
            new: 99,
            count: 255,
        },
    );
}
