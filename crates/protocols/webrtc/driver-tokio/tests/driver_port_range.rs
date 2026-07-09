//! Integration tests for the UDP port range configuration.
//!
//! Validates:
//! - The driver binds within the configured port range.
//! - Invalid port range configurations cause startup failure with clear errors.
//! - When a port in the range is occupied, the driver tries the next one.
//! - Released ports can be reused on subsequent driver starts.

use std::time::Duration;

use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_driver_tokio::{spawn_driver, UdpPortRange, WebRtcDriverConfig};
use tokio::net::UdpSocket;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_port_range_binds_inside_configured_range() {
    let cancel = CancellationToken::new();
    let range = UdpPortRange {
        min: 19000,
        max: 19010,
    };
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(range),
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start with valid port range");

    let bound_port = handle.local_udp_addr().port();
    assert!(
        bound_port >= range.min && bound_port <= range.max,
        "bound port {bound_port} must be within [{}, {}]",
        range.min,
        range.max
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_port_range_rejects_invalid_bounds() {
    // min > max
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(UdpPortRange {
            min: 20000,
            max: 19000,
        }),
        ..Default::default()
    };
    let result = spawn_driver(config, cancel.clone()).await;
    assert!(result.is_err(), "min > max must fail");
    let err = result.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    let msg = err.to_string();
    assert!(
        msg.contains("udp_port_min") && msg.contains("udp_port_max"),
        "error message should mention both bounds: {msg}"
    );
    cancel.cancel();

    // min == 0
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(UdpPortRange { min: 0, max: 100 }),
        ..Default::default()
    };
    let result = spawn_driver(config, cancel.clone()).await;
    assert!(result.is_err(), "min == 0 must fail");
    let err = result.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        err.to_string().contains("udp_port_min"),
        "error should mention udp_port_min: {}",
        err
    );
    cancel.cancel();

    // max == 0
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(UdpPortRange { min: 1, max: 0 }),
        ..Default::default()
    };
    let result = spawn_driver(config, cancel.clone()).await;
    assert!(result.is_err(), "max == 0 must fail");
    cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_port_range_skips_occupied_port() {
    // Occupy the first port in the range, then verify the driver
    // binds to the next available one.
    let blocker = UdpSocket::bind("127.0.0.1:19100").await.unwrap();
    let blocker_port = blocker.local_addr().unwrap().port();
    assert_eq!(blocker_port, 19100);

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(UdpPortRange {
            min: 19100,
            max: 19105,
        }),
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should skip occupied port and bind next");

    let bound_port = handle.local_udp_addr().port();
    assert_ne!(
        bound_port, blocker_port,
        "driver must not bind to the occupied port"
    );
    assert!(
        (19101..=19105).contains(&bound_port),
        "bound port {bound_port} must be in [19101, 19105]"
    );

    cancel.cancel();
    drop(blocker);
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_port_range_all_occupied_returns_error() {
    // Occupy all ports in a small range.
    let _b1 = UdpSocket::bind("127.0.0.1:19200").await.unwrap();
    let _b2 = UdpSocket::bind("127.0.0.1:19201").await.unwrap();
    let _b3 = UdpSocket::bind("127.0.0.1:19202").await.unwrap();

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(UdpPortRange {
            min: 19200,
            max: 19202,
        }),
        ..Default::default()
    };

    let result = spawn_driver(config, cancel.clone()).await;
    assert!(result.is_err(), "all ports occupied must fail");
    let err = result.err().unwrap();
    // The error should be an address-in-use or similar I/O error.
    assert!(
        err.kind() == std::io::ErrorKind::AddrInUse
            || err.kind() == std::io::ErrorKind::AddrNotAvailable,
        "unexpected error kind: {:?} ({})",
        err.kind(),
        err
    );
    cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_port_range_released_port_can_be_reused() {
    // Start a driver on a single-port range, stop it, then start
    // another driver on the same range — the port should be reusable.
    let range = UdpPortRange {
        min: 19300,
        max: 19300,
    };

    let cancel1 = CancellationToken::new();
    let config1 = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(range),
        ..Default::default()
    };
    let handle1 = spawn_driver(config1, cancel1.clone())
        .await
        .expect("first driver should bind");
    assert_eq!(handle1.local_udp_addr().port(), 19300);

    // Stop the first driver and wait for the socket to be released.
    cancel1.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(handle1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Start a second driver on the same single-port range.
    let cancel2 = CancellationToken::new();
    let config2 = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: Some(range),
        ..Default::default()
    };
    let handle2 = spawn_driver(config2, cancel2.clone())
        .await
        .expect("second driver should reuse the released port");
    assert_eq!(handle2.local_udp_addr().port(), 19300);

    cancel2.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_port_range_none_uses_listen_udp_port() {
    // When no port range is configured, the driver uses the port
    // from listen_udp (port 0 = OS-assigned ephemeral).
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        udp_port_range: None,
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start with no port range");

    // OS-assigned port should be non-zero.
    assert_ne!(handle.local_udp_addr().port(), 0);

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
