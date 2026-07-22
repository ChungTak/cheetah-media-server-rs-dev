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
