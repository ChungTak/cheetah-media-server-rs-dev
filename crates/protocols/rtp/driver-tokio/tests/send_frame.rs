use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, FrameOrigin, MediaKind, Timebase, TrackId,
};
use cheetah_rtp_core::{
    RtpClientSpec, RtpConnectionType, RtpPayloadMode, RtpSendFrame, RtpTrackFilter,
    RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{start_driver, RtpDriverCommand, RtpDriverConfig};
use cheetah_runtime_api::CancellationToken;
use std::time::Duration;

#[tokio::test]
async fn test_send_frame_delivers_udp_rtp_to_peer() {
    let cancel = CancellationToken::new();

    // Destination UDP socket: this is the "peer" that should receive the RTP packet.
    let recv_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = recv_socket.local_addr().unwrap();

    // Driver UDP listen address.
    let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    // Driver TCP listen address (required but not used in this test).
    let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    let config = RtpDriverConfig {
        listen_udp: udp_addr,
        listen_tcp: tcp_addr,
        listen_rtcp_udp: None,
        write_queue_capacity: 10,
        read_buffer_size: 4096,
        session_idle_timeout_ms: 5000,
        max_sessions: 5,
        tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
        max_rtp_len_cap: 65536,
    };

    let handle = start_driver(config, cancel.clone());

    let session_key = "send/test".to_string();
    let ssrc = 12345u32;
    let spec = RtpClientSpec {
        session_key: session_key.clone(),
        destination: dest_addr,
        ssrc,
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::SendOnly,
        tcp_conn_id: None,
        connection_type: Some(RtpConnectionType::UdpActive),
        track_filter: RtpTrackFilter::All,
    };

    handle
        .send_command(RtpDriverCommand::CreateClient(spec))
        .await;

    // Wait for the session to be created before sending a frame.
    let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
        .await
        .unwrap()
        .unwrap();
    match event {
        cheetah_rtp_core::RtpCoreEvent::SessionCreated {
            session_key: sk, ..
        } => {
            assert_eq!(sk, "send/test");
        }
        _ => panic!("Expected SessionCreated event for sender"),
    }

    let frame = AVFrame {
        track_id: TrackId(0),
        media_kind: MediaKind::Video,
        codec: CodecId::H264,
        format: FrameFormat::CanonicalH26x,
        pts: 0,
        dts: 0,
        timebase: Timebase::new(1, 90_000),
        pts_us: 0,
        dts_us: 0,
        duration: 0,
        duration_us: 0,
        flags: FrameFlags::KEY,
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x00, 0x00, 0x01, 0xE0]),
        side_data: Default::default(),
        origin: FrameOrigin::Relay,
    };

    handle
        .send_command(RtpDriverCommand::SendFrame(Box::new(RtpSendFrame {
            session_key: session_key.clone(),
            frame,
        })))
        .await;

    let mut buf = vec![0u8; 2048];
    let (len, from) = tokio::time::timeout(Duration::from_secs(5), recv_socket.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(from.ip(), udp_addr.ip());
    assert!(len > 12, "expected a valid RTP packet");

    let rtp = cheetah_codec::RtpPacket::parse(&buf[..len])
        .expect("received data should be a valid RTP packet");
    assert_eq!(rtp.header.ssrc, ssrc);
    assert_eq!(
        rtp.header.payload_type, 96,
        "PS payload should use dynamic PT 96"
    );

    cancel.cancel();
}
