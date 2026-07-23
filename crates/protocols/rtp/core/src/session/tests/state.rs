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

fn ps_datagram(ssrc: u32, seq: u16, source: SocketAddr) -> RtpDatagram {
    RtpDatagram {
        source,
        data: ps_packet(ssrc, seq).encode(),
        received_at_ms: 0,
    }
}

fn audio_frame() -> AVFrame {
    AVFrame::new(
        TrackId(1),
        MediaKind::Audio,
        CodecId::G711U,
        FrameFormat::G711Packet,
        0,
        0,
        Timebase::new(1, 8000),
        Bytes::from(vec![0xD5; 160]),
    )
}

fn state_changed_event(
    outputs: &[RtpCoreOutput],
    session_key: &str,
    expected_old: RtpSessionState,
    expected_new: RtpSessionState,
) -> bool {
    outputs.iter().any(|o| {
        matches!(
            o,
            RtpCoreOutput::Event(RtpCoreEvent::SessionStateChanged {
                session_key: k,
                old_state,
                new_state,
            }) if k == session_key && *old_state == expected_old && *new_state == expected_new
        )
    })
}

#[test]
fn test_recvonly_session_transitions_inactive_to_receiving() {
    let mut core = RtpCore::new(10, 5000);
    let key = "recv-only".to_string();
    let ssrc = 1000u32;
    let source = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc, 100, source)));
    assert!(state_changed_event(
        &outputs,
        &key,
        RtpSessionState::Inactive,
        RtpSessionState::Receiving
    ));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Receiving
    );
}

#[test]
fn test_sendonly_session_transitions_inactive_to_sending() {
    let mut core = RtpCore::new(10, 5000);
    let key = "send-only".to_string();
    let ssrc = 2000u32;
    let dest = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
        RtpClientSpec {
            session_key: key.clone(),
            destination: dest,
            ssrc,
            payload_mode: RtpPayloadMode::RawAudio,
            transport_mode: RtpTransportMode::SendOnly,
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key: key.clone(),
            frame: audio_frame(),
        },
    )));
    assert!(state_changed_event(
        &outputs,
        &key,
        RtpSessionState::Inactive,
        RtpSessionState::Sending
    ));
    assert!(outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::SendUdp(_))));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Sending
    );
}

#[test]
fn test_sendrecv_session_transitions_inactive_to_sendrecv_on_ingress() {
    let mut core = RtpCore::new(10, 5000);
    let key = "sendrecv-ingress".to_string();
    let ssrc = 3000u32;
    let source = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::SendRecv,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc, 100, source)));
    assert!(state_changed_event(
        &outputs,
        &key,
        RtpSessionState::Inactive,
        RtpSessionState::SendRecv
    ));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::SendRecv
    );
}

#[test]
fn test_sendrecv_session_transitions_inactive_to_sendrecv_on_egress() {
    let mut core = RtpCore::new(10, 5000);
    let key = "sendrecv-egress".to_string();
    let ssrc = 4000u32;
    let dest = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
        RtpClientSpec {
            session_key: key.clone(),
            destination: dest,
            ssrc,
            payload_mode: RtpPayloadMode::RawAudio,
            transport_mode: RtpTransportMode::SendRecv,
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key: key.clone(),
            frame: audio_frame(),
        },
    )));
    assert!(state_changed_event(
        &outputs,
        &key,
        RtpSessionState::Inactive,
        RtpSessionState::SendRecv
    ));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::SendRecv
    );
}

#[test]
fn test_voicetalk_upgrades_recvonly_to_talk_immediately() {
    let mut core = RtpCore::new(10, 5000);
    let key = "recv/talk".to_string();
    let ssrc = 5000u32;
    let dest = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
        RtpClientSpec {
            session_key: key.clone(),
            destination: dest,
            ssrc,
            payload_mode: RtpPayloadMode::RawAudio,
            transport_mode: RtpTransportMode::SendRecv,
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: Some(RtpConnectionType::VoiceTalk),
            source_policy: None,
            track_filter: RtpTrackFilter::OnlyAudio,
        },
    )));
    assert!(state_changed_event(
        &outputs,
        &key,
        RtpSessionState::Inactive,
        RtpSessionState::Talk
    ));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Talk
    );
}

#[test]
fn test_voicetalk_keeps_talk_after_ingress_and_egress() {
    let mut core = RtpCore::new(10, 5000);
    let key = "recv/talk2".to_string();
    let ssrc = 6000u32;
    let source = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();
    let dest = "127.0.0.1:15061".parse::<SocketAddr>().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(ssrc),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    // First packet moves RecvOnly session to Receiving.
    let _ = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc, 100, source)));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Receiving
    );

    // VoiceTalk upgrade should move to Talk, not SendRecv.
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
        RtpClientSpec {
            session_key: key.clone(),
            destination: dest,
            ssrc,
            payload_mode: RtpPayloadMode::RawAudio,
            transport_mode: RtpTransportMode::SendRecv,
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: Some(RtpConnectionType::VoiceTalk),
            source_policy: None,
            track_filter: RtpTrackFilter::OnlyAudio,
        },
    )));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Talk
    );

    // Egress audio should keep it in Talk.
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key: key.clone(),
            frame: audio_frame(),
        },
    )));
    assert!(!state_changed_event(
        &outputs,
        &key,
        RtpSessionState::Talk,
        RtpSessionState::Talk
    ));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Talk
    );

    // More ingress should also keep it in Talk.
    let outputs = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc, 101, source)));
    assert!(!state_changed_event(
        &outputs,
        &key,
        RtpSessionState::Talk,
        RtpSessionState::Talk
    ));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Talk
    );
}

#[test]
fn test_stop_command_transitions_to_closed() {
    let mut core = RtpCore::new(10, 5000);
    let key = "stop-session".to_string();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(7000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::StopSession(
        key.clone(),
    )));
    // Terminal state is signaled by `SessionClosed` (and `CloseSession`) rather than a
    // separate `SessionStateChanged` so downstream consumers have a single lifecycle event.
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { session_key, .. }) if session_key == &key
    )));
    assert!(outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::CloseSession(k) if k == &key)));
    assert!(!core.sessions.contains_key(&key));
}

#[test]
fn test_idle_timeout_transitions_to_closed() {
    let mut core = RtpCore::new(10, 1000);
    let key = "idle-timeout".to_string();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(8000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    // Baseline activity on the first tick, then exceed the 1000ms idle window.
    let _ = core.handle_input(RtpCoreInput::Tick { now_ms: 1 });
    let outputs = core.handle_input(RtpCoreInput::Tick { now_ms: 2002 });
    assert!(outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionClosed { session_key, .. }) if session_key == &key
    )));
    assert!(outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::CloseSession(k) if k == &key)));
    assert!(!core.sessions.contains_key(&key));
}

#[test]
fn test_sendframe_in_recvonly_does_not_change_state() {
    let mut core = RtpCore::new(10, 5000);
    let key = "recv-only-no-send".to_string();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        RtpServerSpec {
            session_key: key.clone(),
            ssrc: Some(9000),
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::RecvOnly,
            packet_duration_ms: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key: key.clone(),
            frame: audio_frame(),
        },
    )));
    assert!(!outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionStateChanged { .. })
    )));
    assert!(!outputs
        .iter()
        .any(|o| matches!(o, RtpCoreOutput::SendUdp(_))));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Inactive
    );
}

#[test]
fn test_ingress_in_sendonly_does_not_change_state() {
    let mut core = RtpCore::new(10, 5000);
    let key = "send-only-no-recv".to_string();
    let ssrc = 10000u32;
    let source = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();
    let dest = "127.0.0.1:15061".parse::<SocketAddr>().unwrap();

    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
        RtpClientSpec {
            session_key: key.clone(),
            destination: dest,
            ssrc,
            payload_mode: RtpPayloadMode::Ps,
            transport_mode: RtpTransportMode::SendOnly,
            packet_duration_ms: None,
            tcp_conn_id: None,
            connection_type: None,
            source_policy: None,
            track_filter: RtpTrackFilter::All,
        },
    )));

    let outputs = core.handle_input(RtpCoreInput::UdpPacket(ps_datagram(ssrc, 100, source)));
    assert!(!outputs.iter().any(|o| matches!(
        o,
        RtpCoreOutput::Event(RtpCoreEvent::SessionStateChanged { .. })
    )));
    assert_eq!(
        core.sessions.get(&key).unwrap().state,
        RtpSessionState::Inactive
    );
}
