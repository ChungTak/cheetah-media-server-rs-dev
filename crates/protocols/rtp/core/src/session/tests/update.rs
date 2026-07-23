use super::*;

#[test]
fn test_update_session_advances_generation_and_ssrc_index() {
    let mut core = RtpCore::new(10, 30_000);
    let key = "recv/update".to_string();
    let spec = RtpServerSpec {
        session_key: key.clone(),
        ssrc: Some(1000),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        packet_duration_ms: None,
        connection_type: None,
        source_policy: None,
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
        source_policy: None,
    }));

    let mut updated = false;
    for output in outputs {
        if let RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated {
            session_key,
            generation,
            ssrc,
            payload_type,
            pause_check,
            ..
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
        packet_duration_ms: None,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };
    let spec_b = RtpServerSpec {
        session_key: "b".to_string(),
        ssrc: Some(2000),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        packet_duration_ms: None,
        connection_type: None,
        source_policy: None,
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
        source_policy: None,
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
        source_policy: None,
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
        packet_duration_ms: None,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: key.clone(),
        expected_generation: 1,
        ssrc: None,
        payload_type: Some(96),
        pause_check: None,
        source_policy: None,
    }));

    let mut updated = false;
    for output in outputs {
        if let RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated {
            session_key,
            generation,
            ssrc,
            payload_type,
            pause_check,
            ..
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
        packet_duration_ms: None,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(spec)));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::UpdateSession {
        session_key: key.clone(),
        expected_generation: 1,
        ssrc: Some(1000),
        payload_type: None,
        pause_check: None,
        source_policy: None,
    }));
    let updated = outputs.iter().find_map(|o| match o {
        RtpCoreOutput::Event(RtpCoreEvent::SessionUpdated { generation, .. }) => Some(*generation),
        _ => None,
    });
    assert_eq!(updated, Some(1));
}
