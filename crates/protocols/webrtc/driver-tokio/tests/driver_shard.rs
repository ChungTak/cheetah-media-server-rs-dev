//! Driver multi-shard plumbing smoke tests.
//!
//! Phase 02 follow-up (`plans-27-webrtc-zlm2/phase-02-driver-multithread-shard.md`).
//!
//! These tests exercise the public-facing pieces of the multi-shard
//! groundwork that landed alongside the documentation changes:
//!
//! * `WebRtcDriverConfig::driver_shards` is honoured by the handle
//!   (`shard_count()`).
//! * The handle exposes a non-empty `shard_stats()` snapshot whose
//!   length matches `shard_count()`.
//! * Creating a session registers it in the global route directory.
//! * Closing a session removes it from the directory.
//! * The route directory's address bindings are populated when an
//!   inbound packet finds a session.
//!
//! The current driver still has a single owner shard internally, but
//! the public API is shaped so the multi-shard front-end can drop in
//! without churning callers. These tests guard the API contract.

use std::time::Duration;

use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_core::{WebRtcCloseReason, WebRtcSessionId, WebRtcSessionRole};
use cheetah_webrtc_driver_tokio::{
    spawn_driver, ShardId, WebRtcDriverCommand, WebRtcDriverConfig, WebRtcDriverEvent,
    WebRtcSessionSpec,
};

fn fixture_offer() -> String {
    include_str!("fixtures/minimal_offer.sdp").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_shards_zero_resolves_to_at_least_one() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 0,
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone()).await.unwrap();

    let count = handle.shard_count();
    assert!(
        count >= 1,
        "driver_shards=0 should resolve to >= 1 (got {count})"
    );
    let stats = handle.shard_stats();
    assert_eq!(
        stats.len(),
        count,
        "shard_stats length should match shard_count"
    );
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_shards_explicit_value_is_returned() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };

    let handle = spawn_driver(config, cancel.clone()).await.unwrap();

    assert_eq!(handle.shard_count(), 4);
    assert_eq!(handle.shard_stats().len(), 4);
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_create_session_registers_owner_shard_in_directory() {
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 1,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let directory = handle.route_directory();

    let session_id = WebRtcSessionId::new(101);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    // Wait for the answer to confirm the command landed and the
    // session registered.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => break,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }

    let shard = directory.lookup_session(session_id);
    assert_eq!(
        shard,
        Some(ShardId::new(0)),
        "session must be registered on shard 0"
    );

    let stats = handle.stats_snapshot();
    assert_eq!(stats.shard_count, 1);
    assert!(stats.route_directory.sessions >= 1);

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::SessionClosed {
                session_id: sid, ..
            })) if sid == session_id => break,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }

    assert_eq!(directory.lookup_session(session_id), None);
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_shard_selector_distributes_sessions_by_id() {
    // Phase 02 follow-up second round: when `driver_shards > 1`,
    // `WebRtcDriverHandle::shard_selector().pick(id)` must distribute
    // ids across all shards. The selector is stable and pure, so we
    // can verify with a small sweep without spinning up sessions.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let selector = handle.shard_selector();
    let mut hits = [0usize; 4];
    for i in 0..256 {
        hits[selector.pick_no_loads(WebRtcSessionId::new(i)).as_usize()] += 1;
    }
    for (i, h) in hits.iter().enumerate() {
        assert!(*h > 0, "shard {i} got zero sessions");
    }
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_shard_loads_track_active_sessions() {
    // Driver creates a session, then closes it. The per-shard load
    // table should reflect the +1 / -1 transition for the session's
    // owner shard.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 2,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let selector = handle.shard_selector();
    let session_id = WebRtcSessionId::new(7);
    let owner = selector.pick_no_loads(session_id);

    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => break,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }

    let stats = handle.shard_stats();
    assert_eq!(stats.len(), 2);
    let loaded = stats
        .iter()
        .find(|s| s.shard_id == owner)
        .expect("owner shard present");
    assert_eq!(
        loaded.session_count, 1,
        "shard {owner} should hold exactly one session"
    );

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::SessionClosed {
                session_id: sid, ..
            })) if sid == session_id => break,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }

    let stats = handle.shard_stats();
    let after = stats
        .iter()
        .find(|s| s.shard_id == owner)
        .expect("owner shard present");
    assert_eq!(
        after.session_count, 0,
        "shard {owner} should drop back to zero after close"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_drain_within_returns_true_when_no_sessions() {
    // Phase 02 follow-up: `drain_within(timeout)` should return `true`
    // immediately when there are no active sessions. Operators rely on
    // this short-circuit to avoid sleeping on graceful shutdown when
    // the driver was idle.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let drained = handle.drain_within(Duration::from_millis(500)).await;
    assert!(drained, "drain should succeed for an idle driver");
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_drain_within_waits_for_session_close() {
    // Open a session, then issue StopSession concurrently with
    // drain_within; the drain should resolve `true` once the close
    // propagates.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let session_id = WebRtcSessionId::new(11);
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;
    // Wait for the answer so we know the session landed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => break,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(handle.session_count(), 1);

    let drain_handle = {
        let h = handle.clone();
        tokio::spawn(async move { h.drain_within(Duration::from_secs(2)).await })
    };

    // Stop the session and drain the events so SessionClosed lands.
    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::SessionClosed {
                session_id: sid, ..
            })) if sid == session_id => break,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }

    let drained = drain_handle.await.unwrap();
    assert!(
        drained,
        "drain should resolve true after the session closes"
    );
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
