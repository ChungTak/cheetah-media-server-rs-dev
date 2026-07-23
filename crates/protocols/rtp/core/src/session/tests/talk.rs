use super::*;

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
        packet_duration_ms: None,
        connection_type: None,
        source_policy: None,
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
        packet_duration_ms: None,
        tcp_conn_id: None,
        connection_type: Some(RtpConnectionType::VoiceTalk),
        source_policy: None,
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
fn test_voice_talk_raw_audio_timestamp_sequence_ssrc_continuous() {
    // Raw G.711 talkback must emit a continuous RTP timeline: same SSRC, incrementing
    // sequence numbers, and timestamps that advance by the number of samples sent.
    let mut core = RtpCore::new(10, 30_000);
    let session_key = "recv/talk/cam".to_string();
    let ssrc = 7777u32;
    let peer = "127.0.0.1:15060".parse::<SocketAddr>().unwrap();

    let server_spec = RtpServerSpec {
        session_key: session_key.clone(),
        ssrc: Some(ssrc),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        packet_duration_ms: None,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateServer(
        server_spec,
    )));

    let client_spec = RtpClientSpec {
        session_key: session_key.clone(),
        destination: peer,
        ssrc,
        payload_mode: RtpPayloadMode::RawAudio,
        transport_mode: RtpTransportMode::SendRecv,
        packet_duration_ms: Some(20),
        tcp_conn_id: None,
        connection_type: Some(RtpConnectionType::VoiceTalk),
        source_policy: None,
        track_filter: RtpTrackFilter::OnlyAudio,
    };
    let _ = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::CreateClient(
        client_spec,
    )));

    fn audio_frame(payload: Vec<u8>, pts_us: i64) -> AVFrame {
        AVFrame::new(
            TrackId(1),
            MediaKind::Audio,
            CodecId::G711U,
            FrameFormat::G711Packet,
            pts_us,
            pts_us,
            Timebase::new(1, 8000),
            Bytes::from(payload),
        )
    }

    // First frame: 40 ms of audio at 8 kHz -> 320 samples/bytes. With a 20 ms packet
    // duration, the packetizer splits it into two 160-byte RTP packets.
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key: session_key.clone(),
            frame: audio_frame(vec![0xD5; 320], 0),
        },
    )));
    let first_packets: Vec<RtpPacket> = outputs
        .into_iter()
        .filter_map(|o| match o {
            RtpCoreOutput::SendUdp(udp) => RtpPacket::parse(&udp.data),
            _ => None,
        })
        .collect();
    assert_eq!(first_packets.len(), 2, "expected two 20ms G.711 packets");
    assert_eq!(first_packets[0].header.ssrc, ssrc);
    assert_eq!(first_packets[1].header.ssrc, ssrc);
    assert_eq!(first_packets[0].header.payload_type, 0);
    assert_eq!(first_packets[0].header.sequence_number, 1);
    assert_eq!(first_packets[1].header.sequence_number, 2);
    assert_eq!(first_packets[0].header.timestamp, 0);
    assert_eq!(first_packets[1].header.timestamp, 160);
    assert_eq!(first_packets[0].payload.len(), 160);
    assert_eq!(first_packets[1].payload.len(), 160);

    // Second frame: 20 ms. It should start where the previous frame left off, not
    // restart at the frame's pts (which is still 0).
    let outputs = core.handle_input(RtpCoreInput::Command(RtpCoreCommand::SendFrame(
        RtpSendFrame {
            session_key: session_key.clone(),
            frame: audio_frame(vec![0xD5; 160], 0),
        },
    )));
    let second_packets: Vec<RtpPacket> = outputs
        .into_iter()
        .filter_map(|o| match o {
            RtpCoreOutput::SendUdp(udp) => RtpPacket::parse(&udp.data),
            _ => None,
        })
        .collect();
    assert_eq!(second_packets.len(), 1);
    assert_eq!(second_packets[0].header.ssrc, ssrc);
    assert_eq!(second_packets[0].header.sequence_number, 3);
    assert_eq!(second_packets[0].header.timestamp, 320);
}
