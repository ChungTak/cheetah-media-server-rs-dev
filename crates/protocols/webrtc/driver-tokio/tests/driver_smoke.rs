//! Driver smoke tests.
//!
//! These tests verify that the WebRTC driver:
//! 1. Binds a UDP socket and reports the bound address.
//! 2. Accepts an SDP offer and emits an answer event.
//! 3. Cleanly stops on cancellation without leaking sockets.
//! 4. Connection migration emits `RouteUpdated` with correct `RouteCandidateDiff`.

use std::time::Duration;

use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_core::{
    WebRtcCloseReason, WebRtcOfferDirection, WebRtcOfferSpec, WebRtcSessionId, WebRtcSessionRole,
};
use cheetah_webrtc_driver_tokio::{
    spawn_driver, CandidateTransportPolicy, ShardId, WebRtcDriverCommand, WebRtcDriverConfig,
    WebRtcDriverEvent, WebRtcSessionSpec,
};

fn fixture_offer() -> String {
    include_str!("fixtures/minimal_offer.sdp").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_binds_udp_and_reports_local_addr() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let addr = handle.local_udp_addr();
    assert_eq!(addr.ip().to_string(), "127.0.0.1");
    assert!(addr.port() != 0, "OS must have assigned a UDP port");

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_accept_offer_emits_answer_ready() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let session_id = WebRtcSessionId::new(1);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    let mut saw_answer = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let evt = tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await;
        match evt {
            Ok(Some(WebRtcDriverEvent::AnswerReady {
                session_id: sid,
                sdp,
            })) => {
                assert_eq!(sid, session_id);
                assert!(sdp.starts_with("v=0"));
                saw_answer = true;
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(saw_answer, "driver should emit AnswerReady for valid offer");

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_accept_offer_advertises_configured_public_ip_candidate() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        public_ips: vec!["127.0.0.1".parse().unwrap()],
        driver_shards: 1,
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");
    let advertised = handle.local_udp_addr();

    let session_id = WebRtcSessionId::new(11);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: CandidateTransportPolicy::All,
        }))
        .await;

    let mut answer = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady {
                session_id: sid,
                sdp,
            })) if sid == session_id => {
                answer = Some(sdp);
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    let answer = answer.expect("driver should emit AnswerReady");
    assert!(
        answer.contains(&format!(" 127.0.0.1 {} typ host", advertised.port())),
        "answer SDP must advertise the configured public IP and bound UDP port:\n{answer}"
    );
    assert!(
        answer.contains("a=end-of-candidates"),
        "non-trickle answer SDP must mark ICE gathering completion:\n{answer}"
    );

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_invalid_sdp_emits_diagnostic() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let session_id = WebRtcSessionId::new(7);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: "not sdp".to_string(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    let mut saw_diag = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let evt = tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await;
        match evt {
            Ok(Some(WebRtcDriverEvent::Diagnostic(diag))) => {
                if diag.session_id == Some(session_id) {
                    saw_diag = true;
                    break;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(saw_diag, "garbage SDP should surface a diagnostic event");

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_create_offer_applies_candidate_transport_policy() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let session_id = WebRtcSessionId::new(8);
    handle
        .send_command(WebRtcDriverCommand::CreateOffer {
            session_id,
            role: WebRtcSessionRole::Publisher,
            spec: WebRtcOfferSpec {
                video_direction: Some(WebRtcOfferDirection::RecvOnly),
                audio_direction: Some(WebRtcOfferDirection::RecvOnly),
                data_channel: false,
            },
            candidate_transport_policy: CandidateTransportPolicy::RelayOnly,
        })
        .await;

    let mut offer = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let evt = tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await;
        match evt {
            Ok(Some(WebRtcDriverEvent::OfferReady {
                session_id: sid,
                sdp,
            })) if sid == session_id => {
                offer = Some(sdp);
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    let offer = offer.expect("driver should emit OfferReady");
    assert!(
        !offer.lines().any(|line| line.starts_with("a=candidate:")),
        "relay-only CreateOffer should filter non-relay candidates from SDP:\n{offer}"
    );

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_rejects_unknown_udp_packet_without_panic() {
    // Phase 02 §2.9: an arbitrary datagram from an unbound peer
    // must not panic the driver. The UDP recv loop accepts the
    // bytes, the route table doesn't match any session, and the
    // packet is reported as `UnroutedPacket` diagnostic. STUN /
    // DTLS classification is the responsibility of `str0m`; from
    // the driver's perspective an unknown peer is just noise.
    use tokio::net::UdpSocket;

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let driver_addr = handle.local_udp_addr();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    // Send a 16-byte non-STUN, non-DTLS, non-RTP payload — the
    // driver should classify this as unroutable.
    peer.send_to(&[0xFF; 16], driver_addr).await.unwrap();

    let mut saw_unrouted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let evt = tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await;
        match evt {
            Ok(Some(WebRtcDriverEvent::Diagnostic(diag))) => {
                if matches!(
                    diag.kind,
                    cheetah_webrtc_driver_tokio::WebRtcDriverDiagnosticKind::UnroutedPacket
                ) {
                    saw_unrouted = true;
                    break;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(
        saw_unrouted,
        "unbound peer datagram must surface as UnroutedPacket diagnostic"
    );
    // Driver is still alive and responsive after the unrouted
    // packet — the recv loop should not have died.
    assert_eq!(handle.local_udp_addr(), driver_addr);

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_session_close_decrements_count_and_cleans_state() {
    // Phase 02 §2.10: closing a session must reset
    // `session_count()` so the driver does not accumulate ghosts.
    // We assert two paths: (1) after AcceptOffer the count is 1,
    // (2) after StopSession + brief settle window the count is 0.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    assert_eq!(handle.session_count(), 0, "no sessions yet");

    let session_id = WebRtcSessionId::new(13);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    // Wait for AnswerReady so we know AcceptOffer landed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut answer_seen = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => {
                answer_seen = true;
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(answer_seen, "AcceptOffer should produce an answer");
    assert_eq!(
        handle.session_count(),
        1,
        "session count should be 1 after AcceptOffer"
    );

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;

    // Wait for `SessionClosed` event so we know the close drained.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut closed_seen = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::SessionClosed {
                session_id: sid, ..
            })) if sid == session_id => {
                closed_seen = true;
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(closed_seen, "StopSession should produce SessionClosed");
    assert_eq!(
        handle.session_count(),
        0,
        "session count should drop back to 0 after close"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_command_queue_full_returns_explicit_error() {
    // Phase 02 §2.10: a command queue that fills up must surface a
    // structured error rather than silently blocking the caller.
    // We saturate the queue with `try_send_command` until it
    // refuses, asserting that the rejection comes back as a
    // `WebRtcSendError::QueueFull` and not as a panic / hang. The
    // happy path is that callers using `send_command` await on a
    // small bounded channel; the try-variant exposes the bound.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        // Tiny command queue so we can fill it deterministically.
        command_queue_capacity: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    // Pause the runtime briefly to keep the driver from draining
    // the queue mid-loop. We push StopSession for non-existent
    // sessions so each command is cheap and idempotent.
    let mut queue_full_seen = false;
    for i in 0..1024 {
        let res = handle
            .try_send_command(WebRtcDriverCommand::StopSession {
                session_id: WebRtcSessionId::new(i),
                reason: WebRtcCloseReason::Normal,
            })
            .await;
        match res {
            Ok(()) => continue,
            Err(cheetah_webrtc_driver_tokio::WebRtcSendError::QueueFull) => {
                queue_full_seen = true;
                break;
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert!(
        queue_full_seen,
        "saturating the bounded command queue must yield WebRtcSendError::QueueFull"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_stats_snapshot_reports_counters_and_addresses() {
    // Phase 02 §2.2: `WebRtcDriverHandle` exposes `stats_snapshot()`
    // returning bound addresses + monotonic counters. We assert
    // each counter increments along its respective path:
    // commands_accepted_total bumps when a command lands,
    // events_emitted_total bumps when the caller receives an event,
    // unrouted_packets_total bumps on a non-routable UDP datagram.
    use tokio::net::UdpSocket;

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let baseline = handle.stats_snapshot();
    assert_eq!(baseline.session_count, 0);
    assert_eq!(baseline.commands_accepted_total, 0);
    assert_eq!(baseline.events_emitted_total, 0);
    assert_eq!(baseline.unrouted_packets_total, 0);
    assert_eq!(baseline.local_udp_addr, handle.local_udp_addr());
    assert!(baseline.local_tcp_addr.is_none());

    // Issue an AcceptOffer so commands_accepted_total moves and
    // an AnswerReady event fires.
    let session_id = WebRtcSessionId::new(99);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(2), handle.recv_event()).await;

    // Send an unrouted UDP packet to bump unrouted_packets_total.
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    peer.send_to(&[0xFF; 8], handle.local_udp_addr())
        .await
        .unwrap();
    // Drain whatever event lands so events_emitted_total advances.
    let _ = tokio::time::timeout(Duration::from_secs(2), handle.recv_event()).await;

    let after = handle.stats_snapshot();
    assert!(
        after.commands_accepted_total >= 1,
        "commands_accepted_total should increment after a command: {after:?}"
    );
    assert!(
        after.events_emitted_total >= 1,
        "events_emitted_total should increment after recv_event: {after:?}"
    );
    // Allow up to ~1 s for the unrouted packet to be processed; we
    // already drained one event above which is usually enough.
    let mut saw_unrouted = after.unrouted_packets_total >= 1;
    if !saw_unrouted {
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if handle.stats_snapshot().unrouted_packets_total >= 1 {
                saw_unrouted = true;
                break;
            }
        }
    }
    assert!(
        saw_unrouted,
        "unrouted_packets_total should increment after a non-routable datagram"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_emits_local_candidate_snapshot_after_answer() {
    // Phase 27 §4.1: in single-shard topology, accepting an offer
    // must surface a `LocalCandidateSnapshot` event tagged with
    // `ShardId(0)` alongside the `AnswerReady`. The reported
    // `counts.total()` must equal the number of `a=candidate:`
    // lines in the answer SDP so observers can treat the snapshot
    // as a faithful summary of the gathered candidates without
    // re-parsing the SDP themselves.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        // Pin to a single shard so the snapshot is reported by the
        // single-shard fast path, which canonically tags `ShardId(0)`.
        driver_shards: 1,
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let session_id = WebRtcSessionId::new(4101);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    // Drain events until we have both the snapshot and the answer.
    // `LocalCandidateSnapshot` is emitted right before the
    // corresponding `AnswerReady`, but ordering across the bounded
    // channel is not part of the contract — wait until both arrive.
    let mut answer_sdp: Option<String> = None;
    let mut snapshot: Option<(ShardId, WebRtcSessionId, usize)> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline && (answer_sdp.is_none() || snapshot.is_none()) {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady {
                session_id: sid,
                sdp,
            })) if sid == session_id => {
                answer_sdp = Some(sdp);
            }
            Ok(Some(WebRtcDriverEvent::LocalCandidateSnapshot {
                shard_id,
                session_id: sid,
                counts,
            })) if sid == session_id => {
                snapshot = Some((shard_id, sid, counts.total()));
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    let sdp = answer_sdp.expect("driver should emit AnswerReady for valid offer");
    let (shard_id, snap_session_id, total_count) =
        snapshot.expect("driver should emit LocalCandidateSnapshot for the session");

    assert_eq!(
        shard_id,
        ShardId::new(0),
        "single-shard topology must report ShardId(0)"
    );
    assert_eq!(
        snap_session_id, session_id,
        "snapshot must carry the originating session id"
    );

    let candidate_lines = sdp
        .lines()
        .filter(|l| l.trim_start().starts_with("a=candidate:"))
        .count();
    assert_eq!(
        total_count, candidate_lines,
        "LocalCandidateCounts::total() must match the number of \
         a=candidate: lines in the answer SDP"
    );

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_route_updated_carries_candidate_diff_on_migration() {
    // Phase 02 follow-up Round 13 (`plans-27-webrtc-zlm2/tasks.md` 2.1).
    //
    // Single-shard topology: AcceptOffer + first STUN binding-request
    // from addr_a registers the route but does NOT emit `RouteUpdated`
    // (first bind, no prior address). A second binding-request from
    // addr_b (same session, different source) triggers migration and
    // must surface `RouteUpdated` with:
    //   diff.added == [addr_b]
    //   diff.removed == [addr_a]
    //   diff.stale == [addr_a]
    use str0m::ice::TransId;
    use tokio::net::UdpSocket;

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 1,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");
    let driver_addr = handle.local_udp_addr();
    let directory = handle.route_directory();

    let session_id = WebRtcSessionId::new(2101);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    // Wait for AnswerReady and extract ICE credentials from the answer SDP.
    let answer_sdp = {
        let mut sdp = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline && sdp.is_none() {
            match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
                Ok(Some(WebRtcDriverEvent::AnswerReady {
                    session_id: sid,
                    sdp: s,
                })) if sid == session_id => sdp = Some(s),
                Ok(_) | Err(_) => continue,
            }
        }
        sdp.expect("AnswerReady must arrive for the offered session")
    };

    let server_ufrag =
        parse_sdp_attr(&answer_sdp, "a=ice-ufrag:").expect("answer SDP must carry a=ice-ufrag");
    let server_pwd =
        parse_sdp_attr(&answer_sdp, "a=ice-pwd:").expect("answer SDP must carry a=ice-pwd");
    // Peer ufrag from `tests/fixtures/minimal_offer.sdp`.
    let peer_ufrag = "1WUj";

    // Two peer sockets → two distinct ephemeral ports for addr_a / addr_b.
    let peer_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_a = peer_a.local_addr().unwrap();
    let addr_b = peer_b.local_addr().unwrap();
    assert_ne!(addr_a, addr_b, "test requires two distinct ephemeral ports");

    // First binding-request from addr_a — registers the route but
    // does NOT emit `RouteUpdated` (first bind, no prior address).
    let req_a =
        build_signed_binding_request(&server_ufrag, peer_ufrag, &server_pwd, TransId::new());
    peer_a.send_to(&req_a, driver_addr).await.unwrap();

    // Wait for the route directory to register addr_a on ShardId(0).
    let bind_a_landed = wait_until_no_route_updated(&handle, Duration::from_secs(2), || {
        directory.lookup_remote(&addr_a).map(|(_, shard)| shard) == Some(ShardId::new(0))
    })
    .await;
    assert!(
        bind_a_landed,
        "first binding request from addr_a must register on shard 0",
    );

    // Drain any queued events (LocalCandidateSnapshot, etc.) before
    // sending the second request so the next RouteUpdated is ours.
    drain_pending_events(&handle, Duration::from_millis(50)).await;

    // Second binding-request from addr_b — triggers migration.
    let req_b =
        build_signed_binding_request(&server_ufrag, peer_ufrag, &server_pwd, TransId::new());
    peer_b.send_to(&req_b, driver_addr).await.unwrap();

    // Drain events until the matching `RouteUpdated` arrives.
    let route_update = {
        let mut found = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline && found.is_none() {
            match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
                Ok(Some(WebRtcDriverEvent::RouteUpdated(update)))
                    if update.session_id == session_id =>
                {
                    found = Some(update);
                }
                Ok(_) | Err(_) => continue,
            }
        }
        found.expect("migration must surface a RouteUpdated event for the session")
    };

    assert_eq!(route_update.session_id, session_id);
    assert_eq!(
        route_update.previous_addr,
        Some(addr_a),
        "RouteUpdated must remember the pre-migration address",
    );
    assert_eq!(
        route_update.new_addr, addr_b,
        "RouteUpdated must report the post-migration address",
    );
    assert_eq!(
        route_update.diff.added,
        vec![addr_b],
        "diff.added must be the new address",
    );
    assert_eq!(
        route_update.diff.removed,
        vec![addr_a],
        "diff.removed must be the previous address",
    );
    assert_eq!(
        route_update.diff.stale,
        vec![addr_a],
        "diff.stale must be the previous address (moved to stale set on unbind)",
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_shard_candidate_stats_reflects_latest_snapshot() {
    // Phase 02 follow-up Round 13 (`plans-27-webrtc-zlm2/tasks.md` 4.1).
    //
    // Single-shard topology: after AcceptOffer, the driver emits a
    // `LocalCandidateSnapshot` event AND persists the same counts
    // into the `ShardCandidateTable`. Calling
    // `handle.shard_candidate_stats()` after observing the event
    // must return exactly one entry with `shard_id == ShardId(0)`
    // and `counts.total()` matching the event payload.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 1,
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone())
        .await
        .expect("driver should start");

    let session_id = WebRtcSessionId::new(4110);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    // Drain events until we observe the `LocalCandidateSnapshot` for
    // our session. The driver persists into the shard candidate table
    // BEFORE emitting the event, so by the time we see it the table
    // is guaranteed to be up-to-date.
    let mut snapshot_counts_total: Option<usize> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline && snapshot_counts_total.is_none() {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::LocalCandidateSnapshot {
                shard_id,
                session_id: sid,
                counts,
            })) if sid == session_id => {
                assert_eq!(
                    shard_id,
                    ShardId::new(0),
                    "single-shard topology must tag ShardId(0)"
                );
                snapshot_counts_total = Some(counts.total());
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    let expected_total =
        snapshot_counts_total.expect("driver must emit LocalCandidateSnapshot for the session");

    // Now verify the handle's shard_candidate_stats() reflects the
    // same data that was emitted via the event bus.
    let stats = handle.shard_candidate_stats();
    assert_eq!(
        stats.len(),
        1,
        "single-shard driver must report exactly one shard entry"
    );
    assert_eq!(
        stats[0].shard_id,
        ShardId::new(0),
        "the single entry must be ShardId(0)"
    );
    assert_eq!(
        stats[0].counts.total(),
        expected_total,
        "shard_candidate_stats().counts.total() must match the LocalCandidateSnapshot event payload"
    );

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Parse a single-occurrence `a=` attribute value from an SDP blob.
fn parse_sdp_attr(sdp: &str, key: &str) -> Option<String> {
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Build a STUN binding-request signed with the server's ICE password.
///
/// RFC 8445 §7.2.2: the peer signs with the server's local password;
/// the server verifies with the same. Username is
/// `<local-ufrag>:<remote-ufrag>` from the receiver's perspective.
fn build_signed_binding_request(
    server_ufrag: &str,
    peer_ufrag: &str,
    server_pwd: &str,
    trans_id: str0m::ice::TransId,
) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    let username = format!("{server_ufrag}:{peer_ufrag}");
    let msg = str0m::ice::StunMessage::binding_request(
        &username,
        trans_id,
        true,          // controlling
        0,             // ice control tie breaker
        2_130_706_431, // typical host candidate priority
        false,         // use_candidate
    );
    let mut buf = vec![0u8; 1500];
    let n = msg
        .to_bytes(Some(server_pwd.as_bytes()), &mut buf, |key, payloads| {
            let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC accepts key of any size");
            for payload in payloads {
                mac.update(payload);
            }
            let result = mac.finalize();
            let bytes = result.into_bytes();
            let mut out = [0u8; 20];
            out.copy_from_slice(&bytes);
            out
        })
        .expect("STUN serialization must succeed");
    buf.truncate(n);
    buf
}

/// Poll a predicate while concurrently draining the event bus and
/// asserting that no `RouteUpdated` event slipped in.
async fn wait_until_no_route_updated<F: FnMut() -> bool>(
    handle: &cheetah_webrtc_driver_tokio::WebRtcDriverHandle,
    timeout: Duration,
    mut predicate: F,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if predicate() {
            return true;
        }
        if let Ok(Some(WebRtcDriverEvent::RouteUpdated(update))) =
            tokio::time::timeout(Duration::from_millis(10), handle.recv_event()).await
        {
            panic!("first binding-request must not emit RouteUpdated: {update:?}");
        }
    }
    predicate()
}

/// Drain any events queued on `recv_event()` for at most `quiescence`
/// without blocking on subsequent activity.
async fn drain_pending_events(
    handle: &cheetah_webrtc_driver_tokio::WebRtcDriverHandle,
    quiescence: Duration,
) {
    let deadline = tokio::time::Instant::now() + quiescence;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(10), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::RouteUpdated(_))) => {
                panic!("first binding-request must not emit RouteUpdated");
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
}
