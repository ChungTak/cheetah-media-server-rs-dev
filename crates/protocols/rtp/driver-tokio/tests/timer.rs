use bytes::Bytes;
use cheetah_codec::{RtpHeader, RtpPacket};
use cheetah_rtp_core::rtcp::{RtcpCompoundPacket, RtcpPacket, RtcpReceiverReport, RtcpReportBlock};
use cheetah_rtp_core::{RtpCoreEvent, RtpSessionCloseReason, RtpTcpFraming};
use cheetah_rtp_driver_tokio::{start_driver, DriverLimits, RtpDriverConfig, RtpDriverHandle};
use cheetah_runtime_api::CancellationToken;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::time::Duration;

/// Advance the paused Tokio clock and yield so the driver has a chance to process timers.
async fn advance_time(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    tokio::task::yield_now().await;
}

async fn recv_event_within(handle: &RtpDriverHandle, max_ms: u64) -> Option<RtpCoreEvent> {
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

async fn recv_from_within(
    socket: &tokio::net::UdpSocket,
    buf: &mut [u8],
    max_ms: u64,
) -> Option<(usize, SocketAddr)> {
    let mut waited = 0u64;
    while waited < max_ms {
        if let Ok(res) = socket.try_recv_from(buf) {
            return Some(res);
        }
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        waited += 10;
    }
    None
}

fn test_config(udp_addr: SocketAddr, tcp_addr: SocketAddr) -> RtpDriverConfig {
    RtpDriverConfig {
        listen_udp: udp_addr,
        listen_tcp: tcp_addr,
        listen_rtcp_udp: None,
        write_queue_capacity: 10,
        read_buffer_size: 4096,
        session_idle_timeout_ms: 300,
        max_sessions: 5,
        tick_interval_ms: 100,
        rtcp_report_interval_ms: 1000,
        tcp_framing: RtpTcpFraming::AutoDetect,
        max_rtp_len_cap: 65536,
        limits: DriverLimits::default(),
    }
}

#[tokio::test(start_paused = true)]
async fn test_rtcp_report_interval_uses_paused_clock() {
    let cancel = CancellationToken::new();

    let temp_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    let mut config = test_config(udp_addr, tcp_addr);
    config.session_idle_timeout_ms = 3_000; // larger than RTCP interval
    let handle = start_driver(config, cancel.clone());

    let ssrc = 0x5555_6666u32;
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

    let event = recv_event_within(&handle, 100)
        .await
        .expect("expected SessionCreated event");
    let session_key = match event {
        RtpCoreEvent::SessionCreated {
            ssrc: s,
            session_key,
            ..
        } if s == ssrc => session_key,
        other => panic!("expected SessionCreated for ssrc {ssrc}, got {other:?}"),
    };

    // Bind the peer RTCP socket on the conventional RTP port + 1.
    let rtcp_client =
        tokio::net::UdpSocket::bind(format!("127.0.0.1:{}", rtp_source.port().saturating_add(1)))
            .await
            .unwrap();

    // Feed an RR so the core learns the peer RTCP address and has received traffic.
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
        .send_to(&rr.encode().unwrap(), udp_addr)
        .await
        .unwrap();

    // First tick at 100ms baselines the RTCP report timer.
    advance_time(100).await;
    // Second tick at 1100ms is exactly one interval after the baseline.
    advance_time(1000).await;

    let mut buf = vec![0u8; 2048];
    let (len, _from) = recv_from_within(&rtcp_client, &mut buf, 100)
        .await
        .expect("expected an RTCP packet on the peer RTCP socket");

    assert!(len >= 4);
    let _ = RtcpCompoundPacket::parse(Bytes::copy_from_slice(&buf[..len]))
        .expect("driver should emit a valid RTCP compound packet");

    cancel.cancel();
    drop(session_key);
}

#[tokio::test(start_paused = true)]
async fn test_idle_timeout_uses_paused_clock() {
    let cancel = CancellationToken::new();

    let temp_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = temp_udp.local_addr().unwrap();
    drop(temp_udp);

    let temp_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = temp_tcp.local_addr().unwrap();
    drop(temp_tcp);

    let mut config = test_config(udp_addr, tcp_addr);
    config.rtcp_report_interval_ms = 60_000; // disable RTCP interference
    let handle = start_driver(config, cancel.clone());

    let ssrc = 0x7777_8888u32;
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

    let event = recv_event_within(&handle, 100)
        .await
        .expect("expected SessionCreated event");
    let expected_key = match event {
        RtpCoreEvent::SessionCreated {
            ssrc: s,
            session_key,
            ..
        } if s == ssrc => session_key,
        other => panic!("expected SessionCreated for ssrc {ssrc}, got {other:?}"),
    };

    // First tick baselines activity at 100ms; second tick at 500ms exceeds the
    // 300ms idle timeout.
    advance_time(100).await;
    advance_time(400).await;

    let event = recv_event_within(&handle, 100)
        .await
        .expect("expected SessionClosed event");
    assert!(
        matches!(
            event,
            RtpCoreEvent::SessionClosed {
                reason: RtpSessionCloseReason::IdleTimeout,
                ref session_key,
                ..
            } if session_key == &expected_key
        ),
        "expected idle timeout for {expected_key}, got {event:?}"
    );

    cancel.cancel();
}
