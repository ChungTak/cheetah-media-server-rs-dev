use bytes::Bytes;
use cheetah_codec::{
    AVFrame, CodecId, FrameFlags, FrameFormat, FrameOrigin, MediaKind, Timebase, TrackId,
};
use cheetah_rtp_core::{
    RtpClientSpec, RtpConnectionType, RtpPayloadMode, RtpSendFrame, RtpSessionCloseReason,
    RtpTrackFilter, RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{start_driver, DriverLimits, RtpDriverCommand, RtpDriverConfig};
use cheetah_runtime_api::CancellationToken;
use std::net::UdpSocket as StdUdpSocket;
use std::time::{Duration, Instant};

fn test_config(udp_addr: std::net::SocketAddr, tcp_addr: std::net::SocketAddr) -> RtpDriverConfig {
    RtpDriverConfig {
        listen_udp: udp_addr,
        listen_tcp: tcp_addr,
        listen_rtcp_udp: None,
        write_queue_capacity: 1024,
        read_buffer_size: 4096,
        session_idle_timeout_ms: 30_000,
        max_sessions: 5,
        tick_interval_ms: 100,
        rtcp_report_interval_ms: 5000,
        tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
        max_rtp_len_cap: 65536,
        limits: DriverLimits::default(),
        udp_port_pool: None,
    }
}

fn make_ps_frame(seq: u64) -> AVFrame {
    let payload = Bytes::from(vec![
        0x00, 0x00, 0x01, 0xBA, // pack_start_code
        0x00, 0x00, 0x01, 0xE0, // video stream id
        0x00, 0x0A, // PES packet length placeholder
        0x80, 0x80, 0x05, // flags
        0x21, 0x00, 0x01, 0x00, 0x01, // PTS
    ]);
    AVFrame {
        track_id: TrackId(0),
        media_kind: MediaKind::Video,
        codec: CodecId::H264,
        format: FrameFormat::CanonicalH26x,
        pts: seq as i64,
        dts: seq as i64,
        timebase: Timebase::new(1, 90_000),
        pts_us: seq as i64,
        dts_us: seq as i64,
        duration: 0,
        duration_us: 0,
        flags: FrameFlags::KEY,
        payload,
        side_data: Default::default(),
        origin: FrameOrigin::Relay,
    }
}

/// Self-loop soak: driver sends PS-over-RTP to a local UDP socket for a configurable duration.
///
/// The duration and interval are controlled by environment variables so the same test can be
/// used for a quick smoke run or a long-haul 24 h soak:
///   SOAK_DURATION_SECS   default 5
///   SOAK_SEND_INTERVAL_MS  default 20
#[tokio::test]
#[ignore = "long-running soak; run with --ignored and set SOAK_DURATION_SECS"]
async fn rtp_send_self_loop_soak() {
    let cancel = CancellationToken::new();

    let recv_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dest_addr = recv_socket.local_addr().unwrap();

    let temp_udp = StdUdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    let config = test_config(udp_addr, tcp_addr);
    let handle = start_driver(config, cancel.clone());

    let session_key = "soak/self-loop".to_string();
    let ssrc = 0x1234_5678u32;
    let spec = RtpClientSpec {
        session_key: session_key.clone(),
        destination: dest_addr,
        ssrc,
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::SendOnly,
        packet_duration_ms: None,
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
    match event {
        cheetah_rtp_core::RtpCoreEvent::SessionCreated {
            session_key: sk, ..
        } => {
            assert_eq!(sk, "soak/self-loop");
        }
        _ => panic!("Expected SessionCreated for sender, got {event:?}"),
    }

    let duration_secs: u64 = std::env::var("SOAK_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let interval_ms: u64 = std::env::var("SOAK_SEND_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let duration = Duration::from_secs(duration_secs);
    let interval = Duration::from_millis(interval_ms);

    let mut sent = 0u64;
    let start = Instant::now();
    let mut next = start + interval;

    let mut recv_buf = vec![0u8; 2048];
    let mut received = 0u64;

    while start.elapsed() < duration {
        let frame = make_ps_frame(sent);
        handle
            .send_command(RtpDriverCommand::SendFrame(Box::new(RtpSendFrame {
                session_key: session_key.clone(),
                frame,
            })))
            .await;
        sent += 1;

        // Drain the receiver socket without blocking.
        while let Ok((len, _)) = recv_socket.try_recv_from(&mut recv_buf) {
            if len > 12 {
                received += 1;
            }
        }

        let now = Instant::now();
        if now < next {
            tokio::time::sleep(next - now).await;
        }
        next += interval;
    }

    // Flush any remaining packets.
    for _ in 0..100 {
        if let Ok((len, _)) = recv_socket.try_recv_from(&mut recv_buf) {
            if len > 12 {
                received += 1;
            }
        } else {
            break;
        }
    }

    handle
        .send_command(RtpDriverCommand::StopSession(session_key.clone()))
        .await;

    // Wait for the close event or a timeout.
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match handle.recv_event().await {
                Some(cheetah_rtp_core::RtpCoreEvent::SessionClosed { reason, .. }) => {
                    assert!(matches!(reason, RtpSessionCloseReason::Stopped));
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
    })
    .await;

    eprintln!("soak finished: sent={sent} received={received} duration={duration_secs}s interval={interval_ms}ms");

    // Tolerate a small backlog in the socket/kernel buffers.
    let tolerance = (sent / 100).max(1);
    assert!(
        received + tolerance >= sent,
        "received {received} of {sent} packets; loss exceeded tolerance"
    );
}
