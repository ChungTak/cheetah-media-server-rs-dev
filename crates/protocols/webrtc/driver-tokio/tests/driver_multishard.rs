//! Multi-shard event loop integration tests.
//!
//! Phase 02 follow-up (`plans-27-webrtc-zlm2/phase-02-driver-multithread-shard.md`).
//!
//! These tests exercise the I/O front-end + per-shard worker
//! topology that activates when `WebRtcDriverConfig::driver_shards >
//! 1`. They focus on routing semantics (the session lands on the
//! selector-chosen shard, the directory is consistent, commands are
//! dispatched to the owner shard) rather than on protocol-state
//! correctness — that is already covered by the single-shard smoke
//! tests in `driver_smoke.rs` / `driver_tcp.rs`.

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

async fn drain_until_answer(handle: &cheetah_webrtc_driver_tokio::WebRtcDriverHandle) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => return,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    panic!("never observed AnswerReady");
}

async fn drain_until_close(
    handle: &cheetah_webrtc_driver_tokio::WebRtcDriverHandle,
    target: WebRtcSessionId,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::SessionClosed { session_id, .. }))
                if session_id == target =>
            {
                return
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    panic!("never observed SessionClosed for {target:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_session_lands_on_selector_chosen_shard() {
    // With driver_shards=4 a session created via AcceptOffer should
    // land on the shard that the public ShardSelector picks for it.
    // The directory must reflect that mapping after the answer fires.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let directory = handle.route_directory();
    let selector = handle.shard_selector();
    let session_id = WebRtcSessionId::new(257);
    let expected = selector.pick_no_loads(session_id);

    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

    drain_until_answer(&handle).await;
    let actual = directory.lookup_session(session_id);
    assert_eq!(
        actual,
        Some(expected),
        "session must land on selector-chosen shard {expected:?}, saw {actual:?}"
    );
    let stats = handle.shard_stats();
    let owner_load = stats
        .iter()
        .find(|s| s.shard_id == expected)
        .expect("owner shard present");
    assert_eq!(
        owner_load.session_count, 1,
        "owner shard must hold one session"
    );

    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;
    drain_until_close(&handle, session_id).await;
    assert_eq!(directory.lookup_session(session_id), None);
    let stats = handle.shard_stats();
    let owner_load = stats
        .iter()
        .find(|s| s.shard_id == expected)
        .expect("owner shard present");
    assert_eq!(
        owner_load.session_count, 0,
        "session count drops to zero on close"
    );
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_concurrent_sessions_distribute_across_shards() {
    // Create 16 sessions with sequential ids and verify they spread
    // across all 4 shards. The hash strategy is deterministic so the
    // distribution is reproducible.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let directory = handle.route_directory();
    let selector = handle.shard_selector();

    // Use ids that hash into all 4 shards. We pre-compute the
    // expected distribution so the test fails loudly if the selector
    // changes.
    let mut sessions: Vec<(WebRtcSessionId, ShardId)> = (1..=16u64)
        .map(|i| {
            let id = WebRtcSessionId::new(i);
            (id, selector.pick_no_loads(id))
        })
        .collect();
    // Pre-condition: ids hash into multiple shards.
    let unique_shards: std::collections::HashSet<ShardId> =
        sessions.iter().map(|(_, s)| *s).collect();
    assert!(
        unique_shards.len() >= 2,
        "test requires ids to fan out across >= 2 shards, saw {unique_shards:?}"
    );

    for (id, _) in &sessions {
        handle
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id: *id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: fixture_offer(),
                candidate_transport_policy:
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
            }))
            .await;
    }
    // Drain until we see all answers. Each shard runs its own loop
    // so answers arrive concurrently; we count ready answers and
    // exit early.
    let mut answers_seen = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while answers_seen < sessions.len() && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => {
                answers_seen += 1;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert_eq!(answers_seen, sessions.len(), "all sessions must answer");

    for (id, expected) in &sessions {
        assert_eq!(directory.lookup_session(*id), Some(*expected));
    }

    // shard_stats should reflect exactly the expected distribution.
    let stats = handle.shard_stats();
    let mut counts = [0usize; 4];
    for (_, shard) in &sessions {
        counts[shard.as_usize()] += 1;
    }
    for (shard_id, expected_count) in counts.iter().enumerate() {
        let observed = stats[shard_id].session_count;
        assert_eq!(
            observed, *expected_count,
            "shard {shard_id} expected {expected_count} sessions, observed {observed}"
        );
    }

    // Cleanup: stop every session; each Stop must reach the owner
    // shard via the front-end's directory lookup, otherwise the
    // counts won't drop back to zero.
    for (id, _) in &sessions {
        handle
            .send_command(WebRtcDriverCommand::StopSession {
                session_id: *id,
                reason: WebRtcCloseReason::Normal,
            })
            .await;
    }
    let mut closes_seen = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while closes_seen < sessions.len() && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::SessionClosed { .. })) => {
                closes_seen += 1;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert_eq!(closes_seen, sessions.len(), "all sessions must close");

    let stats = handle.shard_stats();
    for (shard_id, s) in stats.iter().enumerate() {
        assert_eq!(
            s.session_count, 0,
            "shard {shard_id} must drop to zero after all closes"
        );
    }
    sessions.clear();
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_unrouted_packet_emits_diagnostic_at_front_end() {
    // An UDP datagram that doesn't match any session must surface
    // an UnroutedPacket diagnostic from the front-end and bump the
    // unrouted counter. This is the same contract as single-shard.
    use tokio::net::UdpSocket;

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 2,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let driver_addr = handle.local_udp_addr();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    peer.send_to(&[0xFFu8; 16], driver_addr).await.unwrap();

    // Wait for the diagnostic.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_diag = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::Diagnostic(d))) => {
                if matches!(
                    d.kind,
                    cheetah_webrtc_driver_tokio::WebRtcDriverDiagnosticKind::UnroutedPacket
                ) {
                    saw_diag = true;
                    break;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert!(saw_diag, "front-end must emit an UnroutedPacket diagnostic");
    let stats = handle.stats_snapshot();
    assert!(
        stats.unrouted_packets_total >= 1,
        "front-end must increment unrouted_packets_total"
    );
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_stop_unknown_session_emits_diagnostic() {
    // Sending a StopSession for a session id the directory has
    // never seen must not panic the front-end. The expected
    // observation is a `Lifecycle` diagnostic with "unknown session"
    // in the message.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 2,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let session_id = WebRtcSessionId::new(0xDEAD);
    handle
        .send_command(WebRtcDriverCommand::StopSession {
            session_id,
            reason: WebRtcCloseReason::Normal,
        })
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::Diagnostic(d))) => {
                if d.message.contains("unknown session") {
                    saw = true;
                    break;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert!(
        saw,
        "front-end must emit a Lifecycle diagnostic for unknown session id"
    );
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_cancel_emits_shard_stopped_for_each_shard() {
    // Cancelling the driver token must drive the supervisor task
    // into surfacing one ShardStopped event per shard. Operators
    // rely on this signal to differentiate "graceful drain" from
    // "shard panicked".
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 3,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    // Drain initial spawn quiescence.
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut stopped = std::collections::HashSet::new();
    while stopped.len() < 3 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::ShardStopped { shard_id, reason })) => {
                assert!(
                    reason.contains("cancelled") || reason.contains("exited"),
                    "ShardStopped reason should be 'cancelled' or 'exited' on graceful exit, \
                     saw {reason:?}"
                );
                stopped.insert(shard_id);
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(stopped.len(), 3, "expected one ShardStopped per shard");
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_route_counts_track_per_shard() {
    // Per-shard route counters must reflect the actual binding
    // count on each owner shard. We exercise it by creating one
    // session per shard and ensuring the directory observes the
    // session is registered (which is the precondition for route
    // counts to start tracking; actual route binds happen on
    // inbound traffic).
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let selector = handle.shard_selector();

    // Pick session ids that fan out across all 4 shards.
    let mut per_shard: std::collections::HashMap<ShardId, WebRtcSessionId> =
        std::collections::HashMap::new();
    let mut probe = 1u64;
    while per_shard.len() < 4 && probe < 1024 {
        let id = WebRtcSessionId::new(probe);
        let shard = selector.pick_no_loads(id);
        per_shard.entry(shard).or_insert(id);
        probe += 1;
    }
    assert_eq!(per_shard.len(), 4, "could not find 4 ids across all shards");

    for id in per_shard.values() {
        handle
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id: *id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: fixture_offer(),
                candidate_transport_policy:
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
            }))
            .await;
    }
    let mut answers = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while answers < 4 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => {
                answers += 1;
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(answers, 4);
    // Without inbound packets we expect active_routes == 0 on every
    // shard; the test verifies that the per-shard counter exists
    // and starts at zero, not that it has bound any peer addresses
    // (which would require a real STUN binding request to land).
    let stats = handle.shard_stats();
    assert_eq!(stats.len(), 4);
    for s in &stats {
        // session_count must reflect the AcceptOffer; route counts
        // start at zero until the first inbound packet binds.
        assert_eq!(s.session_count, 1, "shard {} session_count", s.shard_id);
        assert_eq!(s.active_routes, 0, "shard {} active_routes", s.shard_id);
        assert_eq!(s.stale_routes, 0, "shard {} stale_routes", s.shard_id);
    }
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_evict_shard_drops_directory_and_load_counters() {
    // Operators call `evict_shard(shard_id)` after observing a
    // ShardStopped(panic). The handle should drop directory
    // entries and reset shard load counters so `shard_stats()`
    // reflects the new reality.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let selector = handle.shard_selector();
    let directory = handle.route_directory();

    // Pick two ids on different shards.
    let mut shard_ids: std::collections::HashMap<ShardId, WebRtcSessionId> =
        std::collections::HashMap::new();
    let mut probe = 1u64;
    while shard_ids.len() < 2 && probe < 1024 {
        let id = WebRtcSessionId::new(probe);
        let shard = selector.pick_no_loads(id);
        shard_ids.entry(shard).or_insert(id);
        probe += 1;
    }
    assert_eq!(shard_ids.len(), 2, "need 2 ids on different shards");
    let (target_shard, target_session) = shard_ids
        .iter()
        .next()
        .map(|(s, id)| (*s, *id))
        .expect("first shard id");
    let (other_shard, other_session) = shard_ids
        .iter()
        .filter(|(s, _)| **s != target_shard)
        .map(|(s, id)| (*s, *id))
        .next()
        .expect("second shard id");

    for id in [target_session, other_session] {
        handle
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id: id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: fixture_offer(),
                candidate_transport_policy:
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
            }))
            .await;
    }
    let mut answers = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while answers < 2 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => {
                answers += 1;
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(answers, 2);

    // Pre-condition: both sessions registered.
    assert_eq!(directory.lookup_session(target_session), Some(target_shard));
    assert_eq!(directory.lookup_session(other_session), Some(other_shard));
    assert_eq!(handle.session_count(), 2);

    // Evict the target shard.
    let evicted = handle.evict_shard(target_shard);
    assert!(
        evicted.sessions >= 1,
        "evict_shard should report at least one session removed, saw {evicted:?}"
    );
    // Target shard's session is gone from the directory.
    assert_eq!(directory.lookup_session(target_session), None);
    // Other shard's session survives.
    assert_eq!(directory.lookup_session(other_session), Some(other_shard));
    // shard_stats reflects the eviction on the target shard.
    let stats = handle.shard_stats();
    let target_load = stats
        .iter()
        .find(|s| s.shard_id == target_shard)
        .expect("target shard present");
    assert_eq!(
        target_load.session_count, 0,
        "target shard count should drop after eviction"
    );
    let other_load = stats
        .iter()
        .find(|s| s.shard_id == other_shard)
        .expect("other shard present");
    assert_eq!(
        other_load.session_count, 1,
        "other shard count should be unchanged"
    );
    // session_count aggregate decremented by the evicted count.
    assert_eq!(handle.session_count(), 1);
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_restart_on_panic_config_does_not_break_graceful_cancel() {
    // Phase 02 Round 9: `shard_restart_on_panic = true` enables
    // auto-eviction on a panicked shard, but graceful cancel /
    // exit paths must stay unchanged. This test confirms the
    // config flag does not regress the cancel path.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 3,
        shard_restart_on_panic: true,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut stopped = std::collections::HashSet::new();
    let mut auto_evict_diag_seen = false;
    while stopped.len() < 3 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::ShardStopped { shard_id, reason })) => {
                assert!(
                    !reason.starts_with("panic:"),
                    "graceful cancel must never produce panic reason, saw {reason:?}"
                );
                stopped.insert(shard_id);
            }
            Ok(Some(WebRtcDriverEvent::Diagnostic(d))) => {
                if d.message.contains("auto-evicted after panic") {
                    auto_evict_diag_seen = true;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(stopped.len(), 3, "expected one ShardStopped per shard");
    assert!(
        !auto_evict_diag_seen,
        "auto-evict diagnostic must not fire on graceful cancel"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_local_candidate_snapshot_carries_owner_shard() {
    // Phase 27 §4.2: in multi-shard topology, every session must
    // observe a `LocalCandidateSnapshot` event whose `shard_id`
    // matches the directory's authoritative owner shard for that
    // session id. We spread N sessions across the shards via the
    // public `ShardSelector` and verify the cross-check holds for
    // each session.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let directory = handle.route_directory();

    // Use ids that fan out across multiple shards. The hash
    // strategy is deterministic, so this distribution is stable
    // across runs.
    let session_ids: Vec<WebRtcSessionId> =
        (0..8u64).map(|i| WebRtcSessionId::new(4200 + i)).collect();

    for id in &session_ids {
        handle
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id: *id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: fixture_offer(),
                candidate_transport_policy:
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
            }))
            .await;
    }

    // Drain events until every session has produced a snapshot.
    // Snapshots and answers are emitted on the same bounded
    // channel, but ordering across shards is not guaranteed; we
    // only care that every session id eventually surfaces one.
    let mut snapshots: std::collections::HashMap<WebRtcSessionId, ShardId> =
        std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while snapshots.len() < session_ids.len() && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::LocalCandidateSnapshot {
                shard_id,
                session_id,
                counts: _,
            })) => {
                snapshots.insert(session_id, shard_id);
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(
        snapshots.len(),
        session_ids.len(),
        "every session must surface a LocalCandidateSnapshot",
    );

    // Cross-check each (session_id, shard_id) against the
    // directory's authoritative owner mapping.
    for id in &session_ids {
        let snap_shard = snapshots
            .get(id)
            .copied()
            .unwrap_or_else(|| panic!("missing snapshot for {id:?}"));
        let owner = directory
            .lookup_session(*id)
            .unwrap_or_else(|| panic!("directory missing owner for {id:?}"));
        assert_eq!(
            snap_shard, owner,
            "snapshot shard for {id:?} must match directory owner",
        );
    }

    // Cleanup: stop every session, then cancel and let the
    // shards drain.
    for id in &session_ids {
        handle
            .send_command(WebRtcDriverCommand::StopSession {
                session_id: *id,
                reason: WebRtcCloseReason::Normal,
            })
            .await;
    }
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_evict_shard_drops_tcp_writers() {
    // Phase 02 follow-up Round 12 (`plans-27-webrtc-zlm2/tasks.md` 2.3).
    //
    // We exercise the cascade from `WebRtcDriverHandle::evict_shard`
    // into the TCP writer registry. The driver does not expose a
    // panic-injection knob, so "constructing a panicking shard" is
    // implemented as a black-box equivalent: open real TCP
    // connections to the driver listener (which puts entries into
    // `TcpWriterRegistry` via `tcp_accept_loop`) and then evict
    // every shard while the entries are live. The expected steady
    // state is `tcp_writer_count() == 0` and `evicted.tcp_writers`
    // summed across all shards equals the number of accepted
    // connections.
    //
    // The companion guarantee for the supervisor's `auto-evict`
    // path — that the lifecycle diagnostic includes the literal
    // `tcp_writers={N}` field — is verified by inspection in
    // `crates/protocols/webrtc/driver-tokio/src/io_front.rs` at the
    // `format!` site that builds the "shard {} auto-evicted after
    // panic: ... tcp_writers={}" string. The Round 9 graceful-cancel
    // test (`multishard_restart_on_panic_config_does_not_break_graceful_cancel`)
    // already locks down that the diagnostic does **not** fire on
    // graceful cancel; combined with this test that pins the field
    // is wired into the public stats struct, we have full coverage
    // of the Round 12 acceptance criteria without an end-to-end
    // panic-injection harness (which the driver does not expose).
    use tokio::net::TcpStream;

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        listen_tcp: Some("127.0.0.1:0".parse().unwrap()),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let selector = handle.shard_selector();
    let directory = handle.route_directory();
    let tcp_addr = handle
        .local_tcp_addr()
        .expect("driver must bind TCP listener");

    // Pick two ids on different shards (mirrors the Round 8 test's
    // pattern). We need at least one real session so the
    // post-eviction directory / load-counter assertions overlap
    // exactly with `multishard_evict_shard_drops_directory_and_load_counters`.
    let mut shard_ids: std::collections::HashMap<ShardId, WebRtcSessionId> =
        std::collections::HashMap::new();
    let mut probe = 1u64;
    while shard_ids.len() < 2 && probe < 1024 {
        let id = WebRtcSessionId::new(probe);
        let shard = selector.pick_no_loads(id);
        shard_ids.entry(shard).or_insert(id);
        probe += 1;
    }
    assert_eq!(shard_ids.len(), 2, "need 2 ids on different shards");
    let (target_shard, target_session) = shard_ids
        .iter()
        .next()
        .map(|(s, id)| (*s, *id))
        .expect("first shard id");
    let (other_shard, other_session) = shard_ids
        .iter()
        .filter(|(s, _)| **s != target_shard)
        .map(|(s, id)| (*s, *id))
        .next()
        .expect("second shard id");

    for id in [target_session, other_session] {
        handle
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id: id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: fixture_offer(),
                candidate_transport_policy:
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
            }))
            .await;
    }

    // Open several TCP connections so the writer registry has live
    // entries to evict. Different ephemeral ports hash to different
    // owner shards inside `shard_for_remote_addr`, so a handful of
    // connections give us a realistic spread without needing to
    // enumerate the hash function from the test.
    const NUM_TCP_CONNECTIONS: usize = 8;
    let mut tcp_streams = Vec::with_capacity(NUM_TCP_CONNECTIONS);
    for _ in 0..NUM_TCP_CONNECTIONS {
        let stream = TcpStream::connect(tcp_addr)
            .await
            .expect("connect to driver TCP listener");
        tcp_streams.push(stream);
    }

    // Drain events until we have observed every TcpAccepted (so
    // every connection landed in the registry) and both AnswerReady
    // results.
    let mut answers_seen = 0;
    let mut accepted_seen = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while (answers_seen < 2 || accepted_seen < NUM_TCP_CONNECTIONS)
        && tokio::time::Instant::now() < deadline
    {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::AnswerReady { .. })) => answers_seen += 1,
            Ok(Some(WebRtcDriverEvent::TcpAccepted { .. })) => accepted_seen += 1,
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(answers_seen, 2, "both sessions must answer");
    assert_eq!(
        accepted_seen, NUM_TCP_CONNECTIONS,
        "every TCP connection must surface a TcpAccepted event",
    );
    assert_eq!(
        handle.tcp_writer_count(),
        NUM_TCP_CONNECTIONS,
        "registry must hold one writer per accepted connection",
    );

    // Pre-condition: both sessions registered (mirrors Round 8).
    assert_eq!(directory.lookup_session(target_session), Some(target_shard));
    assert_eq!(directory.lookup_session(other_session), Some(other_shard));
    assert_eq!(handle.session_count(), 2);

    // Evict every shard and accumulate the cascaded tcp_writers
    // counts. We can't deterministically predict which connection
    // hashed to which owner shard from the integration test (that
    // would mean re-implementing `shard_for_remote_addr` here), so
    // instead we assert the global aggregate: every connection
    // must be evicted exactly once.
    let shard_count = handle.shard_count();
    let mut tcp_writers_evicted_total = 0usize;
    let mut sessions_evicted_total = 0usize;
    for s in 0..shard_count {
        let shard = ShardId::new(s);
        let evicted = handle.evict_shard(shard);
        tcp_writers_evicted_total += evicted.tcp_writers;
        sessions_evicted_total += evicted.sessions;
        if shard == target_shard {
            // Target shard must report at least the session it
            // owned (matches Round 8's contract).
            assert!(
                evicted.sessions >= 1,
                "evicting target shard {target_shard:?} should report at least one session, \
                 saw {evicted:?}",
            );
        }
    }
    assert_eq!(
        tcp_writers_evicted_total, NUM_TCP_CONNECTIONS,
        "summed tcp_writers across all evict_shard calls must equal accepted connection count",
    );
    assert_eq!(
        sessions_evicted_total, 2,
        "summed sessions across all evict_shard calls must equal session count",
    );

    // Steady state: registry empty, directory empty, aggregate
    // counters consistent with Round 8.
    assert_eq!(
        handle.tcp_writer_count(),
        0,
        "tcp_writer_count must drop to zero after evicting every shard",
    );
    assert_eq!(directory.lookup_session(target_session), None);
    assert_eq!(directory.lookup_session(other_session), None);
    assert_eq!(handle.session_count(), 0);
    let stats = handle.shard_stats();
    for s in &stats {
        assert_eq!(
            s.session_count, 0,
            "shard {} session_count must be zero after eviction",
            s.shard_id,
        );
    }

    // Drop the streams *after* eviction so the eviction path is
    // exercised against a populated registry rather than against
    // entries already cleaned up by `tcp_connection_loop` on EOF.
    drop(tcp_streams);
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_shard_candidate_stats_per_shard() {
    // Phase 02 follow-up Round 13 (`plans-27-webrtc-zlm2/tasks.md` 4.2).
    //
    // Multi-shard topology: create one session per shard (4 shards),
    // collect `LocalCandidateSnapshot` events for each, then verify
    // `handle.shard_candidate_stats()` returns 4 entries whose
    // `counts` match the corresponding event payloads. Shard counts
    // must not interfere with each other.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let selector = handle.shard_selector();

    // Pick 4 session ids that fan out across all 4 shards.
    let mut per_shard: std::collections::HashMap<ShardId, WebRtcSessionId> =
        std::collections::HashMap::new();
    let mut probe = 1u64;
    while per_shard.len() < 4 && probe < 1024 {
        let id = WebRtcSessionId::new(probe);
        let shard = selector.pick_no_loads(id);
        per_shard.entry(shard).or_insert(id);
        probe += 1;
    }
    assert_eq!(per_shard.len(), 4, "could not find 4 ids across all shards");

    // Send AcceptOffer for each session (one per shard).
    for id in per_shard.values() {
        handle
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id: *id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: fixture_offer(),
                candidate_transport_policy:
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
            }))
            .await;
    }

    // Drain events until we collect a `LocalCandidateSnapshot` for
    // every session. The driver persists into the shard candidate
    // table BEFORE emitting the event, so by the time we observe
    // all 4 snapshots the table is guaranteed to be up-to-date.
    let mut snapshots: std::collections::HashMap<
        WebRtcSessionId,
        (ShardId, cheetah_webrtc_driver_tokio::LocalCandidateCounts),
    > = std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while snapshots.len() < 4 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::LocalCandidateSnapshot {
                shard_id,
                session_id,
                counts,
            })) => {
                if per_shard.values().any(|id| *id == session_id) {
                    snapshots.insert(session_id, (shard_id, counts));
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(
        snapshots.len(),
        4,
        "every session must surface a LocalCandidateSnapshot, saw {}",
        snapshots.len(),
    );

    // Verify each snapshot's shard_id matches the expected owner.
    for (expected_shard, session_id) in &per_shard {
        let (snap_shard, _) = snapshots
            .get(session_id)
            .unwrap_or_else(|| panic!("missing snapshot for session {session_id:?}"));
        assert_eq!(
            *snap_shard, *expected_shard,
            "snapshot shard for session {session_id:?} must match expected owner shard",
        );
    }

    // Now verify `shard_candidate_stats()` returns 4 entries and
    // each shard's counts match the corresponding event payload.
    let stats = handle.shard_candidate_stats();
    assert_eq!(
        stats.len(),
        4,
        "shard_candidate_stats() must return one entry per shard"
    );

    for (expected_shard, session_id) in &per_shard {
        let (_, event_counts) = snapshots
            .get(session_id)
            .expect("snapshot must exist for session");
        let stat_entry = stats
            .iter()
            .find(|s| s.shard_id == *expected_shard)
            .unwrap_or_else(|| {
                panic!("shard_candidate_stats() must contain entry for shard {expected_shard:?}")
            });
        assert_eq!(
            stat_entry.counts, *event_counts,
            "shard {expected_shard:?}: shard_candidate_stats().counts must match \
             LocalCandidateSnapshot event counts",
        );
    }

    // Verify shard counts don't interfere: each shard's counts
    // should reflect only its own session's candidates, not a sum
    // of all sessions. Since each shard has exactly one session,
    // the total across all shards should equal the sum of
    // individual event totals.
    let stats_total: usize = stats.iter().map(|s| s.counts.total()).sum();
    let events_total: usize = snapshots.values().map(|(_, c)| c.total()).sum();
    assert_eq!(
        stats_total, events_total,
        "sum of shard_candidate_stats totals must equal sum of event totals \
         (no cross-shard interference)",
    );

    // Cleanup.
    for id in per_shard.values() {
        handle
            .send_command(WebRtcDriverCommand::StopSession {
                session_id: *id,
                reason: WebRtcCloseReason::Normal,
            })
            .await;
    }
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_route_updated_carries_candidate_diff_on_migration() {
    // Phase 02 follow-up Round 13 (`plans-27-webrtc-zlm2/tasks.md` 2.2).
    //
    // Multi-shard counterpart of the single-shard `driver_smoke.rs`
    // `driver_route_updated_carries_candidate_diff_on_migration`
    // test. We pin a session onto a non-zero owner shard via the
    // public `ShardSelector`, drive a real STUN binding-request
    // migration on that shard, and assert the same `RouteUpdated`
    // event shape: `diff.added == [addr_b]`, `diff.removed == [addr_a]`,
    // `diff.stale == [addr_a]`. The point is that the per-shard
    // event loop in `runner::run_shard_loop` uses the same
    // `merge_route_diffs` helper as the single-shard fast path, so
    // operators see consistent diff payloads regardless of topology.
    use str0m::ice::TransId;
    use tokio::net::UdpSocket;

    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let driver_addr = handle.local_udp_addr();
    let directory = handle.route_directory();
    let selector = handle.shard_selector();

    // Pick a session id whose hash lands on a non-zero owner shard so
    // the test covers the per-shard migration path rather than only
    // the ShardId(0) fast path. With `driver_shards = 4` the hash
    // strategy is deterministic; probing a small range finds one
    // quickly.
    let mut session_id = WebRtcSessionId::new(1);
    let mut probe = 1u64;
    while selector.pick_no_loads(session_id) == ShardId::new(0) && probe < 1024 {
        probe += 1;
        session_id = WebRtcSessionId::new(probe);
    }
    let owner_shard = selector.pick_no_loads(session_id);
    assert_ne!(
        owner_shard,
        ShardId::new(0),
        "test requires a non-zero owner shard so per-shard migration is exercised",
    );

    // Drive AcceptOffer and capture the answer SDP so we can extract
    // the server-side ICE credentials for crafting binding requests.
    handle
        .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
            session_id,
            role: WebRtcSessionRole::Publisher,
            remote_sdp_offer: fixture_offer(),
            candidate_transport_policy: cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
        }))
        .await;

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
    // Peer ufrag from `tests/fixtures/minimal_offer.sdp`. The server
    // verifies an incoming binding-request integrity using its own
    // local password (RFC 5245 §7.2.2 / RFC 8445: each agent signs
    // with the remote agent's password — so the peer sends signed
    // with the server's pwd, the server verifies with the same).
    let peer_ufrag = "1WUj";

    // Open two peer sockets so we get two distinct ephemeral ports
    // for `addr_a` and `addr_b`. Using two sockets is the cleanest
    // way to spoof a connection migration without having to fight
    // the OS over rebinding a single socket.
    let peer_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_a = peer_a.local_addr().unwrap();
    let addr_b = peer_b.local_addr().unwrap();
    assert_ne!(addr_a, addr_b, "test requires two distinct ephemeral ports",);

    // First binding-request from addr_a — registers the route on the
    // owner shard but does NOT emit `RouteUpdated` because there is
    // no prior address bound for this session. The runner only emits
    // the event when `is_migration` is true, which requires a
    // `previous_addr` distinct from the current source.
    let req_a =
        build_signed_binding_request(&server_ufrag, peer_ufrag, &server_pwd, TransId::new());
    peer_a.send_to(&req_a, driver_addr).await.unwrap();

    // Wait for the route directory to register addr_a on the owner
    // shard so the migration check that follows has something to
    // observe. The directory write is on the same per-shard task as
    // the bind, so once `lookup_remote(addr_a)` resolves the bind
    // has landed and there cannot be a `RouteUpdated` racing in
    // earlier than that.
    let bind_a_landed = wait_until_no_route_updated(&handle, Duration::from_secs(2), || {
        directory.lookup_remote(&addr_a).map(|(_, shard)| shard) == Some(owner_shard)
    })
    .await;
    assert!(
        bind_a_landed,
        "first binding request from addr_a must register on owner shard {owner_shard:?}",
    );

    // While addr_a was binding the driver may have surfaced
    // `LocalCandidateSnapshot` and other events on the bus. Drain
    // anything queued before sending addr_b's request so we can
    // assert the next `RouteUpdated` is the one we triggered.
    drain_pending_events(&handle, Duration::from_millis(50)).await;

    // Second binding-request from addr_b — the runner now sees a
    // `previous_addr == Some(addr_a)`, calls `unbind_address(&addr_a)`
    // (producing `removed: [addr_a], stale: [addr_a]`) and then
    // `try_bind_migration(addr_b, ...)` (producing `added: [addr_b]`).
    // The merged diff is what `RouteUpdated` carries.
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

    // Sanity check: the directory should now point addr_b at the
    // owner shard (the same shard as before the migration since
    // sessions never cross shards in this driver).
    let bind_b_landed = wait_until(Duration::from_secs(1), || {
        directory.lookup_remote(&addr_b).map(|(_, shard)| shard) == Some(owner_shard)
    })
    .await;
    assert!(
        bind_b_landed,
        "post-migration directory must point addr_b at the same owner shard {owner_shard:?}",
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Parse a single-occurrence `a=` attribute value from an SDP blob.
///
/// The minimal answer SDP carries one `a=ice-ufrag:` and one
/// `a=ice-pwd:` per m-section but the values are identical across
/// sections (BUNDLE), so picking the first match is sufficient for
/// the migration test that drives a single STUN binding-request.
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
/// The ICE agent on the server side (`is::IceAgent::accepts_message`)
/// verifies binding-request integrity using the server's **local**
/// password (short-term credential mechanism, RFC 8445 §7.2.2). The
/// username follows RFC 8445 §7.2.2 ordering:
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
    // Peer is the controlling agent (offers with `a=setup:actpass`,
    // server defaults to controlled in `is::IceAgent`). Priority is
    // arbitrary for the wire-format check; pick a typical host
    // candidate priority so future debug dumps look realistic.
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

/// Poll a closure until it returns `true` or the timeout expires.
///
/// Used to wait on side-effect propagation (route directory writes,
/// per-shard counter updates) without coupling the test to the
/// specific tokio task scheduling order.
async fn wait_until<F: FnMut() -> bool>(timeout: Duration, mut predicate: F) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if predicate() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    predicate()
}

/// Poll a predicate while concurrently draining the event bus and
/// asserting that no migration event slipped in. The driver's
/// bounded event channel can otherwise wedge the per-shard task
/// that wants to publish bind progress.
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
/// without blocking on subsequent activity. The test calls this
/// between the two STUN binding-requests so the second-phase
/// `RouteUpdated` is the next event of that variant on the bus.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multishard_shard_candidate_stats_clear_on_auto_evict() {
    // Phase 02 follow-up Round 13 (`plans-27-webrtc-zlm2/tasks.md` 4.3).
    //
    // When `shard_restart_on_panic = true` and a shard panics, the
    // supervisor auto-evicts the dead shard. Part of that eviction
    // calls `shard_candidates.clear_shard(shard_id)`, which resets
    // the candidate counts for the panicked shard to zero. This test
    // verifies that:
    // 1. After the panic + auto-evict, the target shard's candidate
    //    counts are all zero in `shard_candidate_stats()`.
    // 2. Other shards' candidate counts are not affected.
    let cancel = CancellationToken::new();
    let config = WebRtcDriverConfig {
        listen_udp: "127.0.0.1:0".parse().unwrap(),
        driver_shards: 4,
        shard_restart_on_panic: true,
        // Use a high max restart count so the supervisor doesn't
        // stop before we observe the auto-evict diagnostic.
        shard_max_restart_count: 3,
        // Minimal backoff so the test doesn't wait long.
        shard_restart_backoff_ms: 50,
        ..Default::default()
    };
    let handle = spawn_driver(config, cancel.clone()).await.unwrap();
    let selector = handle.shard_selector();

    // Pick session ids that land on at least 2 different shards so
    // we can panic one and verify the other is unaffected.
    let mut per_shard: std::collections::HashMap<ShardId, WebRtcSessionId> =
        std::collections::HashMap::new();
    let mut probe = 1u64;
    while per_shard.len() < 2 && probe < 1024 {
        let id = WebRtcSessionId::new(probe);
        let shard = selector.pick_no_loads(id);
        per_shard.entry(shard).or_insert(id);
        probe += 1;
    }
    assert_eq!(per_shard.len(), 2, "need 2 ids on different shards");

    let (target_shard, target_session) = per_shard
        .iter()
        .next()
        .map(|(s, id)| (*s, *id))
        .expect("first shard");
    let (other_shard, other_session) = per_shard
        .iter()
        .filter(|(s, _)| **s != target_shard)
        .map(|(s, id)| (*s, *id))
        .next()
        .expect("second shard");

    // Create sessions on both shards so they produce
    // LocalCandidateSnapshot events and populate the candidate table.
    for id in [target_session, other_session] {
        handle
            .send_command(WebRtcDriverCommand::AcceptOffer(WebRtcSessionSpec {
                session_id: id,
                role: WebRtcSessionRole::Publisher,
                remote_sdp_offer: fixture_offer(),
                candidate_transport_policy:
                    cheetah_webrtc_driver_tokio::CandidateTransportPolicy::All,
            }))
            .await;
    }

    // Drain events until we observe LocalCandidateSnapshot for both
    // sessions. This guarantees the shard candidate table has been
    // populated for both shards.
    let mut snapshots: std::collections::HashMap<
        WebRtcSessionId,
        (ShardId, cheetah_webrtc_driver_tokio::LocalCandidateCounts),
    > = std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while snapshots.len() < 2 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::LocalCandidateSnapshot {
                shard_id,
                session_id,
                counts,
            })) => {
                if session_id == target_session || session_id == other_session {
                    snapshots.insert(session_id, (shard_id, counts));
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert_eq!(
        snapshots.len(),
        2,
        "both sessions must produce a LocalCandidateSnapshot"
    );

    // Pre-condition: shard_candidate_stats() reflects the snapshot
    // events we observed. The counts may be zero if str0m doesn't
    // include candidates in the answer SDP (trickle ICE), but the
    // table entry must exist and match the event payload.
    let stats_before = handle.shard_candidate_stats();
    let target_before = stats_before
        .iter()
        .find(|s| s.shard_id == target_shard)
        .expect("target shard in stats");
    let other_before = stats_before
        .iter()
        .find(|s| s.shard_id == other_shard)
        .expect("other shard in stats");

    // Verify the table matches the event payloads.
    let (_, target_event_counts) = snapshots
        .get(&target_session)
        .expect("target session snapshot");
    let (_, other_event_counts) = snapshots
        .get(&other_session)
        .expect("other session snapshot");
    assert_eq!(
        target_before.counts, *target_event_counts,
        "target shard stats must match event payload before panic"
    );
    assert_eq!(
        other_before.counts, *other_event_counts,
        "other shard stats must match event payload before panic"
    );

    // Inject a panic on the target shard via the test-only
    // PanicShard command.
    handle
        .send_command(WebRtcDriverCommand::PanicShard {
            shard_id: target_shard,
        })
        .await;

    // Wait for the auto-evict diagnostic that confirms the
    // supervisor processed the panic and cleared the shard.
    let mut saw_auto_evict = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), handle.recv_event()).await {
            Ok(Some(WebRtcDriverEvent::Diagnostic(d))) => {
                if d.message.contains("auto-evicted after panic") {
                    saw_auto_evict = true;
                    break;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => continue,
        }
    }
    assert!(
        saw_auto_evict,
        "supervisor must emit 'auto-evicted after panic' diagnostic for the panicked shard"
    );

    // After auto-evict: target shard's candidate counts must be
    // reset to default (all zeros) by clear_shard().
    let stats_after = handle.shard_candidate_stats();
    let target_after = stats_after
        .iter()
        .find(|s| s.shard_id == target_shard)
        .expect("target shard in stats after eviction");
    assert_eq!(
        target_after.counts,
        cheetah_webrtc_driver_tokio::LocalCandidateCounts::default(),
        "target shard candidate counts must be all zeros after auto-evict, got {:?}",
        target_after.counts,
    );

    // Other shard's candidate counts must be unchanged.
    let other_after = stats_after
        .iter()
        .find(|s| s.shard_id == other_shard)
        .expect("other shard in stats after eviction");
    assert_eq!(
        other_after.counts, other_before.counts,
        "other shard candidate counts must be unaffected by the target shard's panic"
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
