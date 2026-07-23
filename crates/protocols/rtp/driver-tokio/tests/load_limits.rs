//! Load/overload tests for the Tokio RTP driver.
//!
//! Tokio RTP 驱动负载限制测试。

use bytes::Bytes;
use cheetah_codec::{RtpHeader, RtpPacket};
use cheetah_rtp_core::{
    RtpCoreEvent, RtpPayloadMode, RtpServerSpec, RtpTrackFilter, RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{start_driver, DriverLimits, RtpDriverConfig, RtpSocketReuse};
use cheetah_runtime_api::CancellationToken;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::time::Duration;

fn test_config(
    udp_addr: SocketAddr,
    tcp_addr: SocketAddr,
    limits: DriverLimits,
) -> RtpDriverConfig {
    RtpDriverConfig {
        listen_udp: udp_addr,
        listen_tcp: tcp_addr,
        listen_rtcp_udp: None,
        write_queue_capacity: 10,
        read_buffer_size: 4096,
        session_idle_timeout_ms: 5000,
        max_sessions: 10,
        tick_interval_ms: 100,
        rtcp_report_interval_ms: 5000,
        tcp_framing: cheetah_rtp_core::RtpTcpFraming::AutoDetect,
        max_rtp_len_cap: 65536,
        limits,
    }
}

fn make_spec(session_key: &str) -> RtpServerSpec {
    RtpServerSpec {
        session_key: session_key.to_string(),
        ssrc: Some(
            session_key
                .bytes()
                .fold(0u32, |a, b| a.wrapping_add(b as u32)),
        ),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        packet_duration_ms: None,
        connection_type: None,
        track_filter: RtpTrackFilter::All,
        source_policy: None,
    }
}

async fn recv_event_within(
    handle: &cheetah_rtp_driver_tokio::RtpDriverHandle,
    max_ms: u64,
) -> Option<RtpCoreEvent> {
    let mut waited = 0u64;
    while waited < max_ms {
        if let Some(event) = handle.try_recv_event().await {
            return Some(event);
        }
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        waited += 10;
    }
    None
}

#[tokio::test]
async fn max_sessions_rejects_excess_create_server() {
    let cancel = CancellationToken::new();

    let temp_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    let limits = DriverLimits {
        max_sessions: 2,
        max_tcp_connections: 0,
        max_incoming_bytes_per_second: 0,
        bytes_rate_window_ms: 1000,
    };
    let handle = start_driver(test_config(udp_addr, tcp_addr, limits), cancel.clone());

    // Two sessions should succeed.
    let addr1 = handle
        .create_server(
            make_spec("live/1"),
            Some("127.0.0.1:0".parse().unwrap()),
            RtpSocketReuse::Exclusive,
        )
        .await
        .expect("first session should bind");
    let _ = recv_event_within(&handle, 200)
        .await
        .expect("SessionCreated for first");

    let addr2 = handle
        .create_server(
            make_spec("live/2"),
            Some("127.0.0.1:0".parse().unwrap()),
            RtpSocketReuse::Exclusive,
        )
        .await
        .expect("second session should bind");
    let _ = recv_event_within(&handle, 200)
        .await
        .expect("SessionCreated for second");

    assert_ne!(addr1, addr2);

    // Third session should be rejected by the driver load limiter.
    let err = handle
        .create_server(
            make_spec("live/3"),
            Some("127.0.0.1:0".parse().unwrap()),
            RtpSocketReuse::Exclusive,
        )
        .await
        .expect_err("third session should exceed limit");
    assert!(
        err.message.contains("limit"),
        "unexpected error: {}",
        err.message
    );

    cancel.cancel();
}

#[tokio::test(start_paused = true)]
async fn max_incoming_bytes_per_second_drops_excess_packets() {
    let cancel = CancellationToken::new();

    let temp_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    // Allow only 100 bytes per second; the first datagram exceeds the budget and is dropped.
    let limits = DriverLimits {
        max_sessions: 0,
        max_tcp_connections: 0,
        max_incoming_bytes_per_second: 100,
        bytes_rate_window_ms: 1000,
    };
    let handle = start_driver(test_config(udp_addr, tcp_addr, limits), cancel.clone());

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
        payload: Bytes::from(vec![0u8; 200]),
    };

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(&rtp.encode(), udp_addr).await.unwrap();

    // The packet is dropped by the load limiter before reaching the core, so no
    // SessionCreated event should be produced.
    let event = recv_event_within(&handle, 200).await;
    assert!(
        event.is_none(),
        "expected no event when byte-rate limit is exceeded, got {event:?}"
    );

    cancel.cancel();
}

#[tokio::test(start_paused = true)]
async fn byte_rate_window_is_scaled_from_per_second_cap() {
    let cancel = CancellationToken::new();

    let temp_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    // 100 bytes/second with a 100 ms window => 10 bytes per window.
    let limits = DriverLimits {
        max_sessions: 0,
        max_tcp_connections: 0,
        max_incoming_bytes_per_second: 100,
        bytes_rate_window_ms: 100,
    };
    let handle = start_driver(test_config(udp_addr, tcp_addr, limits), cancel.clone());

    let ssrc = 0x1111_2222u32;
    let rtp = |payload: Vec<u8>| RtpPacket {
        header: RtpHeader {
            version: 2,
            payload_type: 96,
            sequence_number: 1,
            timestamp: 0,
            ssrc,
            marker: false,
        },
        payload: Bytes::from(payload),
    };

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // A 20-byte datagram exceeds the 10-byte window budget and is dropped.
    client
        .send_to(&rtp(vec![0u8; 20]).encode(), udp_addr)
        .await
        .unwrap();
    let event = recv_event_within(&handle, 200).await;
    assert!(
        event.is_none(),
        "expected packet over per-window budget to be dropped, got {event:?}"
    );

    cancel.cancel();
}
