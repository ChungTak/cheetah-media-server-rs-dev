use bytes::Bytes;
use cheetah_codec::{RtpHeader, RtpPacket};
use cheetah_rtp_core::rtcp::{
    RtcpBye, RtcpCompoundPacket, RtcpPacket, RtcpReceiverReport, RtcpReportBlock,
};
use cheetah_rtp_driver_tokio::{start_driver, RtpDriverConfig};
use cheetah_runtime_api::CancellationToken;
use std::time::Duration;

#[tokio::test]
async fn test_rtcp_mux_on_udp_socket_closes_session() {
    let cancel = CancellationToken::new();

    let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    // No dedicated RTCP port: RTP and RTCP share the same UDP socket.
    let config = RtpDriverConfig {
        listen_udp: udp_addr,
        listen_tcp: tcp_addr,
        listen_rtcp_udp: None,
        write_queue_capacity: 10,
        read_buffer_size: 4096,
        session_idle_timeout_ms: 30_000,
        max_sessions: 5,
        tick_interval_ms: 100,
        rtcp_report_interval_ms: 5000,
        tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
        max_rtp_len_cap: 65536,
    };

    let handle = start_driver(config, cancel.clone());

    let ssrc = 0x1111_2222u32;
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x00]),
    };

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(&rtp.encode(), udp_addr).await.unwrap();

    // Wait for the auto-created session.
    let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event,
        cheetah_rtp_core::RtpCoreEvent::SessionCreated { ssrc: s, .. } if s == ssrc
    ));

    // Send RTCP BYE for the same SSRC on the *same* UDP port.
    let bye = RtcpCompoundPacket {
        packets: vec![RtcpPacket::Bye(RtcpBye {
            ssrcs: vec![ssrc],
            reason: None,
        })],
    };
    client
        .send_to(&bye.encode().unwrap(), udp_addr)
        .await
        .unwrap();

    // The core should close the session as an explicit action.
    let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, cheetah_rtp_core::RtpCoreEvent::SessionClosed { session_key, .. } if session_key.starts_with("live/")),
        "BYE on the muxed RTP/RTCP socket should close the session"
    );

    cancel.cancel();
}

#[tokio::test]
async fn test_rtcp_separate_port_reply_uses_rtcp_address() {
    let cancel = CancellationToken::new();

    let temp_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_rtcp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let rtcp_addr = temp_rtcp.local_addr().unwrap();
    drop(temp_rtcp);

    let temp_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    // Dedicated RTCP port.
    let config = RtpDriverConfig {
        listen_udp: udp_addr,
        listen_tcp: tcp_addr,
        listen_rtcp_udp: Some(rtcp_addr),
        write_queue_capacity: 10,
        read_buffer_size: 4096,
        session_idle_timeout_ms: 30_000,
        max_sessions: 5,
        tick_interval_ms: 100,
        rtcp_report_interval_ms: 5000,
        tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
        max_rtp_len_cap: 65536,
    };

    let handle = start_driver(config, cancel.clone());

    let ssrc = 0x3333_4444u32;
    let rtp = RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(vec![0x00, 0x00, 0x01, 0xBA, 0x00]),
    };

    let rtp_client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_source = rtp_client.local_addr().unwrap();
    rtp_client.send_to(&rtp.encode(), udp_addr).await.unwrap();

    let event = tokio::time::timeout(Duration::from_secs(5), handle.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event,
        cheetah_rtp_core::RtpCoreEvent::SessionCreated { ssrc: s, .. } if s == ssrc
    ));

    // Bind the peer's RTCP socket on the conventional RTP port + 1.
    let rtcp_client =
        tokio::net::UdpSocket::bind(format!("127.0.0.1:{}", rtp_source.port().saturating_add(1)))
            .await
            .unwrap();
    let _rtcp_source = rtcp_client.local_addr().unwrap();

    // Send an RR on the dedicated RTCP port so the core learns the peer RTCP address.
    let rr = RtcpCompoundPacket {
        packets: vec![RtcpPacket::ReceiverReport(RtcpReceiverReport {
            ssrc: 0xAAAA_AAAA,
            report_blocks: vec![RtcpReportBlock {
                ssrc,
                fraction_lost: 0,
                cumulative_lost: 0,
                highest_seq: 1,
                jitter: 0,
                last_sr: 0,
                delay_since_last_sr: 0,
            }],
        })],
    };
    rtcp_client
        .send_to(&rr.encode().unwrap(), rtcp_addr)
        .await
        .unwrap();

    // The core sends a Receiver Report every 5 seconds.
    tokio::time::sleep(Duration::from_millis(5500)).await;

    let mut buf = vec![0u8; 2048];
    let (len, from) = tokio::time::timeout(Duration::from_secs(5), rtcp_client.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();

    assert!(len >= 4, "expected an RTCP packet on the peer RTCP socket");
    assert_eq!(from.port(), rtcp_addr.port());

    // Confirm the outbound packet is a valid RTCP compound packet.
    let _ = RtcpCompoundPacket::parse(Bytes::copy_from_slice(&buf[..len]))
        .expect("driver should emit a valid RTCP compound packet");

    cancel.cancel();
}
