use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, FrameOrigin, MediaKind, Timebase, TrackId,
};
use cheetah_rtp_core::{
    RtpClientSpec, RtpConnectionType, RtpPayloadMode, RtpSendFrame, RtpServerSpec, RtpSessionState,
    RtpTrackFilter, RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{start_driver, RtpDriverCommand, RtpDriverConfig, RtpSocketReuse};
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
        source_policy: None,
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

#[tokio::test]
async fn test_stop_session_drops_egress_for_stopped_sender() {
    let cancel = CancellationToken::new();

    let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

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

    let recv_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = recv_socket.local_addr().unwrap();

    let session_key = "send/stop".to_string();
    let ssrc = 12346u32;
    let spec = RtpClientSpec {
        session_key: session_key.clone(),
        destination: dest_addr,
        ssrc,
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::SendOnly,
        tcp_conn_id: None,
        connection_type: Some(RtpConnectionType::UdpActive),
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };

    handle
        .send_command(RtpDriverCommand::CreateClient(spec))
        .await;

    let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event,
        cheetah_rtp_core::RtpCoreEvent::SessionCreated { .. }
    ));

    handle
        .send_command(RtpDriverCommand::StopSession(session_key))
        .await;

    // Give the stop a moment to propagate and cancel any in-flight sends.
    tokio::time::sleep(Duration::from_millis(50)).await;

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
            session_key: "send/stop".to_string(),
            frame,
        })))
        .await;

    let mut buf = vec![0u8; 2048];
    let result =
        tokio::time::timeout(Duration::from_millis(200), recv_socket.recv_from(&mut buf)).await;
    assert!(
        result.is_err(),
        "egress for a stopped session should be dropped"
    );

    cancel.cancel();
}

#[tokio::test]
async fn test_per_session_cancel_does_not_affect_other_sessions() {
    let cancel = CancellationToken::new();

    let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

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

    let spec1 = RtpServerSpec {
        session_key: "multi/1".to_string(),
        ssrc: Some(7001),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };

    let spec2 = RtpServerSpec {
        session_key: "multi/2".to_string(),
        ssrc: Some(7002),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        connection_type: None,
        source_policy: None,
        track_filter: RtpTrackFilter::All,
    };

    let _addr1 = handle
        .create_server(
            spec1,
            Some("127.0.0.1:0".parse().unwrap()),
            RtpSocketReuse::Exclusive,
        )
        .await
        .expect("create_server should bind");
    let _ = handle.recv_event().await;

    let addr2 = handle
        .create_server(
            spec2,
            Some("127.0.0.1:0".parse().unwrap()),
            RtpSocketReuse::Exclusive,
        )
        .await
        .expect("create_server should bind");
    let _ = handle.recv_event().await;

    handle
        .send_command(RtpDriverCommand::StopSession("multi/1".to_string()))
        .await;

    // Drain the SessionClosed event for multi/1 before sending to multi/2.
    let closed = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        closed,
        cheetah_rtp_core::RtpCoreEvent::SessionClosed {
            ref session_key,
            ..
        } if session_key == "multi/1"
    ));

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp = cheetah_codec::RtpPacket {
        header: cheetah_codec::RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 100,
            ssrc: 7002,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA]),
    };
    client.send_to(&rtp.encode(), addr2).await.unwrap();

    let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            event,
            cheetah_rtp_core::RtpCoreEvent::SessionStateChanged {
                ref session_key,
                new_state: RtpSessionState::Receiving,
                ..
            } if session_key == "multi/2"
        ),
        "stopping one session must not cancel the driver or other sessions: got {event:?}"
    );

    cancel.cancel();
}
