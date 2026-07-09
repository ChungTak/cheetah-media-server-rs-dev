//! Phase 05 follow-up — supervisor↔hub↔bridge integration test.
//!
//! Verifies the three-way lifecycle without a real network or driver:
//!
//! 1. `KeeperTransportFactory` returns one `InMemoryTransport` per
//!    connect; the test holds the paired remote half and plays the
//!    role of the signaling server.
//! 2. The supervisor wraps the transport in a `KeeperHub`, calls
//!    `on_hub_ready`, and pumps `run_dispatcher` in parallel.
//! 3. The observer attaches one peer bridge and runs `run_bridge`
//!    against a `RecordingDriverSink`. The signaling server replies
//!    with answer + bye, which drives the bridge to its terminal
//!    `Bye` state.
//! 4. The supervisor returns `KeeperRemoved` once the test removes
//!    the keeper from the registry.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_core::WebRtcSessionId;
use cheetah_webrtc_module::p2p::{
    bridge::{
        run_bridge, P2pBridgeConfig, P2pBridgeOutcome, RecordingDriverSink, StaticOfferWaiter,
    },
    hub::{KeeperHub, KeeperHubConfig, PeerKey},
    job::{P2pJobConfig, P2pJobKind, P2pJobState},
    message::{P2pMessage, P2pMessageHeader, P2pStreamTuple},
    room::{P2pRoomKeeperConfig, P2pRoomKeeperRegistry, P2pRoomKeeperSnapshot},
    supervisor::{
        run_supervisor_with_hub, KeeperHubObserver, KeeperSupervisorConfig,
        KeeperSupervisorOutcome, KeeperTransportFactory,
    },
    transport::{InMemoryTransport, P2pTransport, P2pTransportError, P2pTransportEvent},
};
use parking_lot::Mutex;

struct PairFactory {
    pending_remotes: Arc<Mutex<Vec<InMemoryTransport>>>,
}

#[async_trait]
impl KeeperTransportFactory for PairFactory {
    type Transport = InMemoryTransport;

    async fn connect(
        &self,
        _snapshot: &P2pRoomKeeperSnapshot,
    ) -> Result<Self::Transport, P2pTransportError> {
        let (local, remote) = InMemoryTransport::pair(8);
        self.pending_remotes.lock().push(remote);
        Ok(local)
    }
}

/// Observer that attaches a single peer bridge per hub, runs it
/// concurrently with the dispatcher, and stores the outcome for the
/// test to inspect. The observer keeps `on_hub_ready` running until
/// the supervisor's `hub_cancel` fires; it does **not** wait for the
/// bridge by itself, because the test wants the supervisor's
/// teardown path (close → cancel) to drive the bridge to completion.
struct OneBridgeObserver {
    peer_key: PeerKey,
    job_config: P2pJobConfig,
    bridge_outcome: Arc<Mutex<Option<P2pBridgeOutcome>>>,
}

#[async_trait]
impl KeeperHubObserver for OneBridgeObserver {
    type Transport = InMemoryTransport;

    async fn on_hub_ready(
        &self,
        _snapshot: P2pRoomKeeperSnapshot,
        hub: Arc<KeeperHub<InMemoryTransport>>,
        hub_cancel: CancellationToken,
    ) {
        let transport = hub.attach(self.peer_key.clone()).expect("attach");
        let driver = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(StaticOfferWaiter {
            sdp: "v=0\r\noffer".into(),
        });
        let cfg = P2pBridgeConfig {
            job: self.job_config.clone(),
            session_id: WebRtcSessionId::new(7777),
            offer_timeout: Duration::from_millis(500),
        };
        let outcome = run_bridge(cfg, transport, driver, waiter, hub_cancel).await;
        *self.bridge_outcome.lock() = Some(outcome);
    }
}

fn keeper_cfg(room: &str) -> P2pRoomKeeperConfig {
    P2pRoomKeeperConfig {
        server_host: "signaling.example.com".into(),
        server_port: 8443,
        room_id: room.into(),
        vhost: None,
        app: Some("live".into()),
        stream: Some("demo".into()),
        ssl: true,
    }
}

fn job_cfg(kind: P2pJobKind, room: &str, peer: &str, transport: &str) -> P2pJobConfig {
    P2pJobConfig {
        kind,
        stream: P2pStreamTuple {
            vhost: "v".into(),
            app: "live".into(),
            stream: "demo".into(),
        },
        local_room_id: peer.into(),
        peer_room_id: room.into(),
        transport_id: transport.into(),
        pending_candidate_cap: 4,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn supervisor_drives_hub_drives_bridge_pull_lifecycle() {
    let registry = Arc::new(P2pRoomKeeperRegistry::default());
    let key = registry.add(keeper_cfg("room1")).unwrap();
    let pending = Arc::new(Mutex::new(Vec::<InMemoryTransport>::new()));
    let factory = PairFactory {
        pending_remotes: pending.clone(),
    };
    let bridge_outcome = Arc::new(Mutex::new(None));
    let observer = Arc::new(OneBridgeObserver {
        peer_key: PeerKey::new("room1", "peer-a", "tr-a"),
        job_config: job_cfg(P2pJobKind::Pull, "room1", "peer-a", "tr-a"),
        bridge_outcome: bridge_outcome.clone(),
    });

    let cancel = CancellationToken::new();
    let supervisor_handle = {
        let registry = registry.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            run_supervisor_with_hub(
                registry,
                key,
                KeeperSupervisorConfig {
                    retry_initial_backoff: Duration::from_millis(50),
                    retry_max_backoff: Duration::from_millis(200),
                    // Single attempt — once the keeper is removed
                    // mid-run, the watchdog cancels the hub and the
                    // supervisor returns `KeeperRemoved`.
                    max_attempts: 1,
                },
                KeeperHubConfig::default(),
                factory,
                observer,
                Arc::new(cheetah_runtime_tokio::TokioRuntime::new()),
                cancel,
            )
            .await
        })
    };

    // Wait for the supervisor to establish a transport (≤ 2s).
    let remote = {
        let mut taken: Option<InMemoryTransport> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Some(r) = pending.lock().pop() {
                taken = Some(r);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        taken.expect("supervisor should establish a transport within 2s")
    };

    // Pretend to be the signaling server.
    match tokio::time::timeout(Duration::from_secs(2), remote.recv())
        .await
        .expect("server should see a check-in within 2s")
        .expect("recv ok")
    {
        P2pTransportEvent::Message(P2pMessage::CheckIn { header, .. }) => {
            let echo_header = P2pMessageHeader {
                room_id: header.room_id.clone(),
                peer_id: header.peer_id.clone(),
                transport_id: header.transport_id.clone(),
            };
            remote
                .send(P2pMessage::Answer {
                    header: echo_header.clone(),
                    sdp: "v=0\r\nanswer".into(),
                })
                .await
                .unwrap();
            remote
                .send(P2pMessage::Bye {
                    header: echo_header,
                    reason: Some("done".into()),
                })
                .await
                .unwrap();
        }
        other => panic!("expected check_in, got {other:?}"),
    }

    // Wait for the bridge to finish (it sees the bye and returns).
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            if bridge_outcome.lock().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let outcome = bridge_outcome.lock().clone().expect("bridge outcome");
    match outcome {
        P2pBridgeOutcome::Completed { final_state } => {
            assert_eq!(final_state, P2pJobState::Bye);
        }
        other => panic!("bridge unexpected: {other:?}"),
    }

    // Remove the keeper. The supervisor's watchdog notices and
    // cancels the hub, which makes the supervisor exit cleanly.
    let _ = registry.remove(key);

    let supervisor_outcome = tokio::time::timeout(Duration::from_secs(3), supervisor_handle)
        .await
        .expect("supervisor should return within 3s")
        .expect("join ok");
    assert!(
        matches!(
            supervisor_outcome,
            KeeperSupervisorOutcome::KeeperRemoved | KeeperSupervisorOutcome::Stopped
        ),
        "expected KeeperRemoved or Stopped, got {supervisor_outcome:?}"
    );
    cancel.cancel();
}
