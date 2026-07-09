//! TCP listener integration tests for the WebRTC driver.
//!
//! These exercise the Phase 02 RFC 4571 framing path:
//!
//! * The driver binds a TCP listener when `listen_tcp` is set.
//! * Inbound TCP connections produce a `TcpAccepted` event before
//!   any frames are forwarded.
//! * RFC 4571 framed bytes are decoded and routed through the same
//!   `route_unbound_packet` path as UDP. We verify this by watching
//!   for an `UnroutedPacket` diagnostic when the framed bytes do not
//!   match any active session — this is the same outcome you would
//!   see from a stray UDP datagram.
//! * Closing the TCP side produces a `TcpClosed` event.
//! * Without `listen_tcp`, `local_tcp_addr()` is `None` and no TCP
//!   accept loop runs.

use std::time::Duration;

use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_driver_tokio::{
    spawn_driver, tcp_encode_frame, WebRtcDriverConfig, WebRtcDriverEvent, WebRtcDriverHandle,
    WebRtcTcpCloseReason,
};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

fn driver_config(listen_tcp: bool) -> WebRtcDriverConfig {
    WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        listen_tcp: if listen_tcp {
            Some("127.0.0.1:0".parse().unwrap())
        } else {
            None
        },
        ..Default::default()
    }
}

async fn drain_one_event(handle: &WebRtcDriverHandle) -> Option<WebRtcDriverEvent> {
    tokio::time::timeout(Duration::from_millis(500), handle.recv_event())
        .await
        .ok()
        .flatten()
}

async fn collect_events(handle: &WebRtcDriverHandle, deadline: Duration) -> Vec<WebRtcDriverEvent> {
    let mut events = Vec::new();
    let until = tokio::time::Instant::now() + deadline;
    while tokio::time::Instant::now() < until {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(e)) => events.push(e),
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    events
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_without_listen_tcp_does_not_bind_tcp() {
    let cancel = CancellationToken::new();
    let handle = spawn_driver(driver_config(false), cancel.clone())
        .await
        .expect("driver should start");

    assert!(
        handle.local_tcp_addr().is_none(),
        "local_tcp_addr must be None when listen_tcp is unset"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_binds_tcp_when_configured_and_emits_accepted_on_connect() {
    let cancel = CancellationToken::new();
    let handle = spawn_driver(driver_config(true), cancel.clone())
        .await
        .expect("driver should start");

    let tcp_addr = handle
        .local_tcp_addr()
        .expect("TCP listener must report a bound address");
    assert_eq!(tcp_addr.ip().to_string(), "127.0.0.1");
    assert_ne!(tcp_addr.port(), 0);

    let _stream = TcpStream::connect(tcp_addr)
        .await
        .expect("connect to TCP listener should succeed");

    let events = collect_events(&handle, Duration::from_millis(500)).await;
    let accepted = events
        .iter()
        .find(|e| matches!(e, WebRtcDriverEvent::TcpAccepted { .. }))
        .expect("TcpAccepted event should be emitted on connect");
    if let WebRtcDriverEvent::TcpAccepted { remote_addr } = accepted {
        assert_eq!(remote_addr.ip().to_string(), "127.0.0.1");
    } else {
        unreachable!();
    }

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_decodes_rfc4571_frames_and_unroutes_unknown_packets() {
    let cancel = CancellationToken::new();
    let handle = spawn_driver(driver_config(true), cancel.clone())
        .await
        .expect("driver should start");

    let tcp_addr = handle.local_tcp_addr().expect("tcp addr");
    let mut stream = TcpStream::connect(tcp_addr)
        .await
        .expect("connect to TCP listener should succeed");

    // Send a syntactically valid RFC 4571 frame whose payload does
    // not match any active WebRTC session. The driver should decode
    // the frame and then surface an `UnroutedPacket` diagnostic
    // because no session accepts it.
    let payload = vec![0xAA, 0xBB, 0xCC, 0xDD];
    let framed = tcp_encode_frame(&payload).expect("encode frame");
    stream
        .write_all(&framed)
        .await
        .expect("write framed packet");
    stream.flush().await.unwrap();

    let events = collect_events(&handle, Duration::from_secs(2)).await;
    let saw_unrouted = events.iter().any(|e| {
        matches!(
            e,
            WebRtcDriverEvent::Diagnostic(d)
                if matches!(
                    d.kind,
                    cheetah_webrtc_driver_tokio::WebRtcDriverDiagnosticKind::UnroutedPacket
                )
        )
    });
    assert!(
        saw_unrouted,
        "RFC 4571 frame whose payload matches no session should produce UnroutedPacket diagnostic"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_emits_tcp_closed_on_peer_eof() {
    let cancel = CancellationToken::new();
    let handle = spawn_driver(driver_config(true), cancel.clone())
        .await
        .expect("driver should start");

    let tcp_addr = handle.local_tcp_addr().expect("tcp addr");
    let stream = TcpStream::connect(tcp_addr)
        .await
        .expect("connect to TCP listener should succeed");
    drop(stream); // peer EOF.

    let events = collect_events(&handle, Duration::from_secs(2)).await;
    let saw_closed = events
        .iter()
        .any(|e| matches!(e, WebRtcDriverEvent::TcpClosed { .. }));
    assert!(saw_closed, "peer EOF should surface a TcpClosed event");

    // Also verify that the close reason is PeerEof, not a framing
    // error or shutdown.
    let close_kind = events.iter().find_map(|e| match e {
        WebRtcDriverEvent::TcpClosed { reason, .. } => Some(reason),
        _ => None,
    });
    assert!(matches!(close_kind, Some(WebRtcTcpCloseReason::PeerEof)));

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_emits_tcp_closed_on_oversize_frame() {
    let cancel = CancellationToken::new();
    let mut config = driver_config(true);
    // Very small max frame so the test can trigger the limit
    // without sending megabytes.
    config.tcp_frame_max_bytes = 8;
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let tcp_addr = handle.local_tcp_addr().expect("tcp addr");
    let mut stream = TcpStream::connect(tcp_addr)
        .await
        .expect("connect to TCP listener should succeed");

    // Length prefix advertises 32 bytes — over the 8-byte cap. The
    // driver should close the connection without panicking.
    stream.write_all(&[0x00, 0x20]).await.unwrap();
    stream.flush().await.unwrap();

    let _accepted = drain_one_event(&handle).await;
    let events = collect_events(&handle, Duration::from_secs(2)).await;
    let close_event = events
        .iter()
        .find(|e| matches!(e, WebRtcDriverEvent::TcpClosed { .. }))
        .expect("oversize frame should produce TcpClosed");
    if let WebRtcDriverEvent::TcpClosed { reason, .. } = close_event {
        assert!(
            matches!(reason, WebRtcTcpCloseReason::FramingError { .. }),
            "expected FramingError, got {reason:?}"
        );
    }

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_emits_tcp_closed_on_idle_timeout() {
    // Phase 02 follow-up: a TCP peer that connects but never sends
    // any bytes must not hold a driver task open forever. The
    // driver closes the connection when no data arrives within
    // `tcp_idle_timeout_ms` and surfaces `IdleTimeout` as the close
    // reason.
    let cancel = CancellationToken::new();
    let mut config = driver_config(true);
    config.tcp_idle_timeout_ms = 250; // tight window for the test.
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let tcp_addr = handle.local_tcp_addr().expect("tcp addr");
    let stream = TcpStream::connect(tcp_addr)
        .await
        .expect("connect to TCP listener should succeed");
    // Hold the stream open without sending anything.
    let _stream_guard = stream;

    let events = collect_events(&handle, Duration::from_secs(2)).await;
    let close_event = events
        .iter()
        .find(|e| matches!(e, WebRtcDriverEvent::TcpClosed { .. }))
        .expect("idle timeout should produce a TcpClosed event");
    if let WebRtcDriverEvent::TcpClosed { reason, .. } = close_event {
        assert!(
            matches!(reason, WebRtcTcpCloseReason::IdleTimeout),
            "expected IdleTimeout, got {reason:?}"
        );
    }

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_idle_timeout_disabled_when_zero() {
    // `tcp_idle_timeout_ms = 0` keeps the legacy behaviour: an idle
    // peer is held open until EOF. We assert that no
    // `IdleTimeout` event fires inside a short window.
    let cancel = CancellationToken::new();
    let mut config = driver_config(true);
    config.tcp_idle_timeout_ms = 0;
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let tcp_addr = handle.local_tcp_addr().expect("tcp addr");
    let _stream = TcpStream::connect(tcp_addr)
        .await
        .expect("connect to TCP listener should succeed");

    let events = collect_events(&handle, Duration::from_millis(500)).await;
    let saw_idle_timeout = events.iter().any(|e| {
        matches!(
            e,
            WebRtcDriverEvent::TcpClosed {
                reason: WebRtcTcpCloseReason::IdleTimeout,
                ..
            }
        )
    });
    assert!(
        !saw_idle_timeout,
        "with tcp_idle_timeout_ms=0 the driver must never emit IdleTimeout"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_handshake_timeout_closes_stuck_session() {
    // Phase 02 follow-up: when ICE/DTLS never completes the driver
    // must force-close the session after `handshake_timeout_ms`.
    // We accept an offer (so the session is registered as
    // handshake-pending) and then never feed network packets — the
    // session can therefore never reach `Lifecycle::Connected`.
    use cheetah_webrtc_core::{WebRtcCloseReason, WebRtcSessionId, WebRtcSessionRole};
    use cheetah_webrtc_driver_tokio::{WebRtcDriverCommand, WebRtcSessionSpec};

    let cancel = CancellationToken::new();
    let mut config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    config.handshake_timeout_ms = 500;
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let session_id = WebRtcSessionId::new(1);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: include_str!("fixtures/minimal_offer.sdp").to_string(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    // The driver's watchdog sweeps once per second. With a 500ms
    // handshake timeout the close fires after roughly 1 second.
    let events = collect_events(&handle, std::time::Duration::from_secs(4)).await;

    let saw_handshake_close = events.iter().any(|e| {
        matches!(
            e,
            WebRtcDriverEvent::SessionClosed {
                reason: WebRtcCloseReason::HandshakeTimeout,
                ..
            }
        )
    });
    assert!(
        saw_handshake_close,
        "handshake watchdog should produce SessionClosed::HandshakeTimeout, got {events:?}"
    );

    cancel.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_handshake_timeout_disabled_when_zero() {
    // `handshake_timeout_ms = 0` disables the watchdog. A session
    // that never connects should still be present in the driver
    // for at least a short window, with no `HandshakeTimeout` close
    // event.
    use cheetah_webrtc_core::{WebRtcCloseReason, WebRtcSessionId, WebRtcSessionRole};
    use cheetah_webrtc_driver_tokio::{WebRtcDriverCommand, WebRtcSessionSpec};

    let cancel = CancellationToken::new();
    let mut config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    config.handshake_timeout_ms = 0;
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let session_id = WebRtcSessionId::new(2);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: include_str!("fixtures/minimal_offer.sdp").to_string(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    let events = collect_events(&handle, std::time::Duration::from_millis(1500)).await;
    let saw_handshake_close = events.iter().any(|e| {
        matches!(
            e,
            WebRtcDriverEvent::SessionClosed {
                reason: WebRtcCloseReason::HandshakeTimeout,
                ..
            }
        )
    });
    assert!(
        !saw_handshake_close,
        "with handshake_timeout_ms=0 the watchdog must never fire"
    );

    cancel.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}
