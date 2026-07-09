//! Phase 05 follow-up — round 8 follow-up: end-to-end test that drives
//! `run_supervisor_with_hub` against the real `WebSocketTransportFactory`
//! and a `tokio-tungstenite` server inside the test process.
//!
//! This test confirms the full chain — supervisor → factory →
//! WebSocket connect → KeeperHub → run_bridge → answer → bye → KeeperRemoved
//! — works without any in-memory shortcuts. It's the closest the
//! workspace can get to a real ZLM interop run without an external
//! peer.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_runtime_api::CancellationToken;
use cheetah_webrtc_core::WebRtcSessionId;
use cheetah_webrtc_module::p2p::{
    bridge::{
        run_bridge, P2pBridgeConfig, P2pBridgeOutcome, RecordingDriverSink, StaticOfferWaiter,
    },
    hub::{KeeperHub, PeerKey},
    job::{P2pJobConfig, P2pJobKind, P2pJobState},
    message::{P2pMessage, P2pMessageHeader, P2pStreamTuple},
    room::{P2pRoomKeeperConfig, P2pRoomKeeperRegistry, P2pRoomKeeperSnapshot},
    supervisor::{
        run_supervisor_with_hub, KeeperHubObserver, KeeperSupervisorConfig, KeeperSupervisorOutcome,
    },
    transport::P2pTransportEvent,
    SignalingUrlPolicy, WebSocketP2pTransport, WebSocketTransportConfig, WebSocketTransportFactory,
};
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

struct OneBridgeObserver {
    peer_key: PeerKey,
    job_config: P2pJobConfig,
    bridge_outcome: Arc<Mutex<Option<P2pBridgeOutcome>>>,
}

#[async_trait]
impl KeeperHubObserver for OneBridgeObserver {
    type Transport = WebSocketP2pTransport;

    async fn on_hub_ready(
        &self,
        _snapshot: P2pRoomKeeperSnapshot,
        hub: Arc<KeeperHub<WebSocketP2pTransport>>,
        hub_cancel: CancellationToken,
    ) {
        let transport = hub.attach(self.peer_key.clone()).expect("attach");
        let driver = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(StaticOfferWaiter {
            sdp: "v=0\r\noffer".into(),
        });
        let cfg = P2pBridgeConfig {
            job: self.job_config.clone(),
            session_id: WebRtcSessionId::new(424242),
            offer_timeout: Duration::from_millis(500),
        };
        let outcome = run_bridge(cfg, transport, driver, waiter, hub_cancel).await;
        *self.bridge_outcome.lock() = Some(outcome);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn supervisor_drives_real_websocket_transport_end_to_end() {
    // Bind a real TCP listener so we get a free port.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();

    // Server task: accept exactly one WebSocket, expect a `check_in`,
    // reply with `answer + bye`, then close.
    let server = tokio::spawn(async move {
        let (stream, _addr) = listener.accept().await.expect("accept ok");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("ws accept");
        // Read until we get a check_in.
        let mut answered = false;
        while !answered {
            match ws.next().await {
                Some(Ok(WsMessage::Text(raw))) => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(raw.as_str()).expect("json");
                    if parsed["type"] == "check_in" {
                        let room_id = parsed["room_id"].as_str().unwrap().to_string();
                        let peer_id = parsed["peer_id"].as_str().unwrap().to_string();
                        let transport_id = parsed["transport_id"].as_str().unwrap().to_string();
                        let answer = serde_json::json!({
                            "type": "answer",
                            // Echo the routing fields so the hub
                            // dispatches to the right peer key.
                            "room_id": room_id,
                            "peer_id": peer_id,
                            "transport_id": transport_id,
                            "sdp": "v=0\r\nanswer",
                        });
                        ws.send(WsMessage::Text(answer.to_string().into()))
                            .await
                            .unwrap();
                        let bye = serde_json::json!({
                            "type": "bye",
                            "room_id": room_id,
                            "peer_id": peer_id,
                            "transport_id": transport_id,
                            "reason": "done",
                        });
                        ws.send(WsMessage::Text(bye.to_string().into()))
                            .await
                            .unwrap();
                        answered = true;
                    }
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => break,
            }
        }
        let _ = ws.close(None).await;
        while ws.next().await.is_some() {}
    });

    // Build the registry + observer.
    let registry = Arc::new(P2pRoomKeeperRegistry::default());
    let key = registry
        .add(P2pRoomKeeperConfig {
            server_host: bound.ip().to_string(),
            server_port: bound.port(),
            room_id: "room42".into(),
            vhost: None,
            app: Some("live".into()),
            stream: Some("demo".into()),
            ssl: false,
        })
        .unwrap();

    let bridge_outcome = Arc::new(Mutex::new(None));
    let observer = Arc::new(OneBridgeObserver {
        peer_key: PeerKey::new("room42", "ringing", "tr1"),
        job_config: P2pJobConfig {
            kind: P2pJobKind::Pull,
            stream: P2pStreamTuple {
                vhost: "v".into(),
                app: "live".into(),
                stream: "demo".into(),
            },
            local_room_id: "ringing".into(),
            peer_room_id: "room42".into(),
            transport_id: "tr1".into(),
            pending_candidate_cap: 4,
        },
        bridge_outcome: bridge_outcome.clone(),
    });

    let factory = WebSocketTransportFactory::new(WebSocketTransportConfig {
        url_policy: SignalingUrlPolicy {
            allow_private_ips: true,
            ..Default::default()
        },
        // Override URL to talk to the test server's path; the room
        // keeper config wouldn't otherwise know the URL.
        url_override: Some(format!("ws://{}/p2p", bound)),
        connect_timeout: Duration::from_secs(2),
        ..Default::default()
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
                    max_attempts: 1,
                },
                Default::default(),
                factory,
                observer,
                cancel,
            )
            .await
        })
    };

    // Wait for the bridge to complete.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if bridge_outcome.lock().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let outcome = bridge_outcome.lock().clone().expect("bridge outcome");
    match outcome {
        P2pBridgeOutcome::Completed { final_state } => {
            assert_eq!(final_state, P2pJobState::Bye);
        }
        other => panic!("bridge unexpected: {other:?}"),
    }

    // Remove the keeper so the supervisor exits cleanly. Note the
    // supervisor may already have observed the server-side close
    // and counted it as one attempt; we accept any of the terminal
    // outcomes as long as the bridge completed cleanly above.
    let _ = registry.remove(key);

    let supervisor_outcome = tokio::time::timeout(Duration::from_secs(5), supervisor_handle)
        .await
        .expect("supervisor return")
        .expect("join ok");
    assert!(
        matches!(
            supervisor_outcome,
            KeeperSupervisorOutcome::KeeperRemoved
                | KeeperSupervisorOutcome::Stopped
                | KeeperSupervisorOutcome::GaveUp { .. }
        ),
        "expected supervisor to terminate cleanly, got {supervisor_outcome:?}"
    );
    cancel.cancel();
    let _ = server.await;

    // Suppress unused-import warnings for transport types used only
    // through the supervisor pipeline.
    let _ = std::any::type_name::<P2pMessage>();
    let _ = std::any::type_name::<P2pMessageHeader>();
    let _ = std::any::type_name::<P2pTransportEvent>();
}
