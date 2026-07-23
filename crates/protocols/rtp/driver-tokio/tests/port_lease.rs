//! Port lease / socket bind tests for the Tokio RTP driver.
//!
//! Tokio RTP 驱动端口租约与 socket 绑定测试。

use cheetah_rtp_core::RtpTcpFraming;
use cheetah_rtp_core::{
    RtpPayloadMode, RtpServerSpec, RtpSourcePolicy, RtpTrackFilter, RtpTransportMode,
};
use cheetah_rtp_driver_tokio::{
    start_driver, DriverLimits, RtpDriverConfig, RtpDriverHandle, RtpSocketReuse,
};
use cheetah_runtime_api::CancellationToken;
use std::net::{SocketAddr, TcpListener, UdpSocket as StdUdpSocket};
use std::time::Duration;

fn test_config(udp_addr: SocketAddr, tcp_addr: SocketAddr) -> RtpDriverConfig {
    RtpDriverConfig {
        listen_udp: udp_addr,
        listen_tcp: tcp_addr,
        listen_rtcp_udp: None,
        write_queue_capacity: 10,
        read_buffer_size: 4096,
        session_idle_timeout_ms: 30_000,
        max_sessions: 10,
        tick_interval_ms: 100,
        rtcp_report_interval_ms: 5_000,
        tcp_framing: RtpTcpFraming::AutoDetect,
        max_rtp_len_cap: 65_536,
        limits: DriverLimits::default(),
    }
}

fn make_spec(session_key: &str) -> RtpServerSpec {
    RtpServerSpec {
        session_key: session_key.to_string(),
        ssrc: Some(0x1111_2222),
        payload_mode: RtpPayloadMode::Ps,
        transport_mode: RtpTransportMode::RecvOnly,
        packet_duration_ms: None,
        connection_type: None,
        source_policy: Some(RtpSourcePolicy::Strict),
        track_filter: RtpTrackFilter::All,
    }
}

async fn create_server(
    handle: &RtpDriverHandle,
    session_key: &str,
    bind_addr: SocketAddr,
    reuse: RtpSocketReuse,
) -> Result<SocketAddr, String> {
    handle
        .create_server(make_spec(session_key), Some(bind_addr), reuse)
        .await
        .map_err(|e| e.message)
}

fn free_udp_and_tcp() -> (SocketAddr, SocketAddr) {
    let udp = StdUdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr().unwrap();
    drop(udp);

    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_addr = tcp.local_addr().unwrap();
    drop(tcp);

    (udp_addr, tcp_addr)
}

#[tokio::test]
async fn explicit_bind_and_stop_releases_port() {
    let cancel = CancellationToken::new();
    let (udp_addr, tcp_addr) = free_udp_and_tcp();
    let handle = start_driver(test_config(udp_addr, tcp_addr), cancel.clone());

    // Use an ephemeral port chosen by the OS.
    let temp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let bind_addr = temp.local_addr().unwrap();
    drop(temp);

    let actual = create_server(
        &handle,
        "recv/release",
        bind_addr,
        RtpSocketReuse::Exclusive,
    )
    .await
    .unwrap();

    assert_eq!(actual, bind_addr);

    handle
        .send_command(cheetah_rtp_driver_tokio::RtpDriverCommand::StopSession(
            "recv/release".to_string(),
        ))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // After the session stops, the socket must be released so an external bind succeeds.
    let _rebound = tokio::net::UdpSocket::bind(bind_addr).await.unwrap();
    cancel.cancel();
}

#[tokio::test]
async fn bind_failure_returns_error_and_does_not_consume_port() {
    let cancel = CancellationToken::new();
    let (udp_addr, tcp_addr) = free_udp_and_tcp();
    let handle = start_driver(test_config(udp_addr, tcp_addr), cancel.clone());

    // Grab a port first so the driver's bind is guaranteed to fail.
    let occupied = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let bind_addr = occupied.local_addr().unwrap();

    let err = create_server(&handle, "recv/fail", bind_addr, RtpSocketReuse::Exclusive)
        .await
        .unwrap_err();
    assert!(
        err.contains("failed to bind UDP socket"),
        "unexpected error: {err}"
    );

    // The driver should still be able to bind a fresh ephemeral port.
    let temp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let next_addr = temp.local_addr().unwrap();
    drop(temp);

    let actual = create_server(&handle, "recv/ok", next_addr, RtpSocketReuse::Exclusive)
        .await
        .unwrap();
    assert_eq!(actual, next_addr);

    cancel.cancel();
}

#[tokio::test]
async fn reuse_shares_socket_until_last_session_stops() {
    let cancel = CancellationToken::new();
    let (udp_addr, tcp_addr) = free_udp_and_tcp();
    let handle = start_driver(test_config(udp_addr, tcp_addr), cancel.clone());

    let temp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let bind_addr = temp.local_addr().unwrap();
    drop(temp);

    let a = create_server(&handle, "recv/reuse-a", bind_addr, RtpSocketReuse::Reuse)
        .await
        .unwrap();
    let b = create_server(&handle, "recv/reuse-b", bind_addr, RtpSocketReuse::Reuse)
        .await
        .unwrap();
    assert_eq!(a, b);

    // Stop one session; the shared socket must remain bound.
    handle
        .send_command(cheetah_rtp_driver_tokio::RtpDriverCommand::StopSession(
            "recv/reuse-a".to_string(),
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let external = tokio::net::UdpSocket::bind(bind_addr).await;
    assert!(
        external.is_err(),
        "shared socket was released while another session held it"
    );

    // Stop the second session; now the socket must be released.
    handle
        .send_command(cheetah_rtp_driver_tokio::RtpDriverCommand::StopSession(
            "recv/reuse-b".to_string(),
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let _rebound = tokio::net::UdpSocket::bind(bind_addr).await.unwrap();
    cancel.cancel();
}

#[tokio::test]
async fn duplicate_session_key_prevents_double_socket() {
    let cancel = CancellationToken::new();
    let (udp_addr, tcp_addr) = free_udp_and_tcp();
    let handle = start_driver(test_config(udp_addr, tcp_addr), cancel.clone());

    let temp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let bind_addr = temp.local_addr().unwrap();
    drop(temp);

    let _ = create_server(&handle, "recv/dup", bind_addr, RtpSocketReuse::Exclusive)
        .await
        .unwrap();

    let err = create_server(&handle, "recv/dup", bind_addr, RtpSocketReuse::Exclusive)
        .await
        .unwrap_err();
    assert!(
        err.contains("already has a bound socket"),
        "unexpected error: {err}"
    );

    // The first session still owns its socket; stopping it should release it.
    handle
        .send_command(cheetah_rtp_driver_tokio::RtpDriverCommand::StopSession(
            "recv/dup".to_string(),
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let _rebound = tokio::net::UdpSocket::bind(bind_addr).await.unwrap();
    cancel.cancel();
}
