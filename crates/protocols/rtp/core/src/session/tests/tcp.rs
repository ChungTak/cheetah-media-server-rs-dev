use super::*;

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
        source_policy: None,
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
        source_policy: None,
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
fn test_rtp_core_tcp_connection_closed_terminates_bound_sessions() {
    // A peer half-close on the TCP connection should immediately close all sessions bound to
    // that connection id, emitting a terminal SessionClosed event with ConnectionClosed reason.
    let mut core = RtpCore::new(10, 30_000);
    let ssrc = 0xDEADBEEFu32;
    let spec = RtpServerSpec {
        session_key: "live/tcp-close".to_string(),
        ssrc: Some(ssrc),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0xAA, 0xBB]),
    };
    let frame = cheetah_codec::encode_tcp_rtp_frame(&rtp);
    let _ = core.handle_input(RtpCoreInput::TcpBytes(crate::types::RtpTcpChunk {
        conn_id: 7,
        data: frame,
        received_at_ms: 0,
    }));

    let outputs = core.handle_input(RtpCoreInput::TcpConnectionClosed {
        conn_id: 7,
        received_at_ms: 1,
    });

    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionClosed {
            session_key,
            reason: RtpSessionCloseReason::ConnectionClosed,
        }) if session_key == "live/tcp-close"
    )));
    assert!(outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::CloseSession(_))));
}

#[test]
fn test_tcp_connection_closed_keeps_send_capable_session_alive() {
    // A peer half-close must not terminate a send-only (or sendrecv) session because the
    // outbound write path may still be draining frames/RTCP.
    let mut core = RtpCore::new(10, 30_000);
    let dest = "127.0.0.1:1234".parse().unwrap();
    let spec = RtpClientSpec {
        session_key: "live/tcp-push".to_string(),
        destination: dest,
        ssrc: 0xC0FFEE,
        payload_mode: RtpPayloadMode::Es,
        transport_mode: RtpTransportMode::SendOnly,
        tcp_conn_id: Some(42),
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(spec)));

    let outputs = core.handle_input(RtpCoreInput::TcpConnectionClosed {
        conn_id: 42,
        received_at_ms: 0,
    });

    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::CloseSession(_))));
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::CloseTcpConnection { conn_id: 42 })));
    // The session should still be present and send-capable.
    assert!(core.sessions.contains_key("live/tcp-push"));
}
