//! Signaling hub: one transport, many P2P bridges.
//!
//! Phase 05 follow-up (round 4): in real deployments a single
//! WebSocket connection to the signaling server carries several
//! concurrent P2P sessions (one per `(room_id, peer_id, transport_id)`
//! tuple). The supervisor brings the connection up and the *hub*
//! multiplexes inbound messages to the right [`run_bridge`] instance.
//!
//! ## Architecture
//!
//! ```text
//! signaling WS  ──▶  KeeperHub.read_loop  ──▶  per-peer mpsc channel
//!                                                ▲
//!                                                │
//!                                  HubInboundSink (used by run_bridge
//!                                  via HubBackedTransport)
//!
//! HubBackedTransport.send  ────▶  hub.send  ────▶  signaling WS
//! ```
//!
//! The hub is **runtime-aware** but **not** schema-aware beyond the
//! envelope keys — it dispatches messages by `(room_id, peer_id,
//! transport_id)` and never inspects the SDP / candidate payload.
//!
//! Test coverage uses `InMemoryTransport::pair` for the underlying
//! signaling channel; production code plugs in a real WebSocket
//! transport.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_runtime_api::CancellationToken;
use futures::channel::mpsc;
use futures::lock::Mutex as AsyncMutex;
use futures::{FutureExt, SinkExt, StreamExt};
use parking_lot::Mutex;
use thiserror::Error;

use super::message::{P2pMessage, P2pMessageHeader};
use super::transport::{P2pTransport, P2pTransportError, P2pTransportEvent};

/// Routing key for hub demultiplexing. Mirrors the wire envelope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerKey {
    pub room_id: String,
    pub peer_id: String,
    pub transport_id: String,
}

impl PeerKey {
    pub fn new(
        room_id: impl Into<String>,
        peer_id: impl Into<String>,
        transport_id: impl Into<String>,
    ) -> Self {
        Self {
            room_id: room_id.into(),
            peer_id: peer_id.into(),
            transport_id: transport_id.into(),
        }
    }

    /// Build a key from a wire envelope. Returns `None` when any of
    /// the three identifying fields is missing — those messages can't
    /// be routed to a specific bridge and the hub drops them with a
    /// diagnostic.
    pub fn from_header(header: &P2pMessageHeader) -> Option<Self> {
        Some(Self {
            room_id: header.room_id.clone()?,
            peer_id: header.peer_id.clone()?,
            transport_id: header.transport_id.clone()?,
        })
    }

    /// Pull the routing key out of a `P2pMessage` envelope.
    pub fn from_message(msg: &P2pMessage) -> Option<Self> {
        let header = match msg {
            P2pMessage::CheckIn { header, .. }
            | P2pMessage::CheckInOk { header, .. }
            | P2pMessage::Offer { header, .. }
            | P2pMessage::Answer { header, .. }
            | P2pMessage::Candidate { header, .. }
            | P2pMessage::Bye { header, .. }
            | P2pMessage::Error { header, .. }
            | P2pMessage::Ping { header }
            | P2pMessage::Pong { header }
            | P2pMessage::RoomList { header, .. } => header,
            P2pMessage::Unknown { .. } => return None,
        };
        Self::from_header(header)
    }
}

/// Hub-level error.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum KeeperHubError {
    #[error("hub already has bridge for {0:?}")]
    DuplicateKey(PeerKey),
    #[error("hub at peer capacity ({0})")]
    CapacityExceeded(usize),
    #[error("hub is closed")]
    Closed,
}

/// Hub configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeeperHubConfig {
    /// Per-peer mpsc channel capacity. Backpressure for inbound
    /// signaling messages routed to a bridge.
    pub peer_channel_capacity: usize,
    /// Maximum number of concurrent peers per hub.
    pub max_peers: usize,
}

impl Default for KeeperHubConfig {
    fn default() -> Self {
        Self {
            peer_channel_capacity: 32,
            max_peers: 1024,
        }
    }
}

/// Hub state keyed by peer. The hub owns the underlying transport so
/// it can serialize sends from multiple bridges (concurrent
/// `send` calls on a `tungstenite::WebSocketStream` would corrupt
/// the framing).
pub struct KeeperHub<T: P2pTransport + 'static> {
    transport: Arc<T>,
    config: KeeperHubConfig,
    inbound: Mutex<HashMap<PeerKey, mpsc::Sender<P2pTransportEvent>>>,
    closed: std::sync::atomic::AtomicBool,
}

impl<T: P2pTransport + 'static> KeeperHub<T> {
    pub fn new(transport: T, config: KeeperHubConfig) -> Arc<Self> {
        Arc::new(Self {
            transport: Arc::new(transport),
            config,
            inbound: Mutex::new(HashMap::new()),
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn config(&self) -> &KeeperHubConfig {
        &self.config
    }

    pub fn peer_count(&self) -> usize {
        self.inbound.lock().len()
    }

    /// Register a new peer. Returns a [`HubBackedTransport`] that
    /// `run_bridge` can consume directly.
    pub fn attach(self: &Arc<Self>, key: PeerKey) -> Result<HubBackedTransport<T>, KeeperHubError> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(KeeperHubError::Closed);
        }
        let mut guard = self.inbound.lock();
        if guard.contains_key(&key) {
            return Err(KeeperHubError::DuplicateKey(key));
        }
        if guard.len() >= self.config.max_peers {
            return Err(KeeperHubError::CapacityExceeded(self.config.max_peers));
        }
        let (tx, rx) = mpsc::channel(self.config.peer_channel_capacity.max(1));
        guard.insert(key.clone(), tx);
        Ok(HubBackedTransport {
            hub: self.clone(),
            key,
            inbound: AsyncMutex::new(rx),
            detached: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Pump the underlying transport and dispatch events to peers.
    /// Returns when the transport closes, the cancel token fires, or
    /// the hub is closed.
    pub async fn run_dispatcher(&self, cancel: CancellationToken) {
        loop {
            if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            let event = {
                let cancelled = cancel.cancelled().fuse();
                let recv = self.transport.recv().fuse();
                futures::pin_mut!(cancelled, recv);
                futures::select_biased! {
                    _ = cancelled => break,
                    res = recv => res,
                }
            };
            match event {
                Ok(P2pTransportEvent::Message(msg)) => {
                    if let Some(key) = PeerKey::from_message(&msg) {
                        let sender = self.inbound.lock().get(&key).cloned();
                        if let Some(mut tx) = sender {
                            let _ = tx.send(P2pTransportEvent::Message(msg)).await;
                        }
                        // Otherwise: no bridge listening — drop. The
                        // signaling spec lets hosts ignore unknown
                        // peers; logging is left to the runtime
                        // adapter.
                    }
                    // Unrouteable messages (no header) are dropped.
                }
                Ok(P2pTransportEvent::Closed) => {
                    self.broadcast_closed().await;
                    break;
                }
                Ok(P2pTransportEvent::Error(reason)) => {
                    self.broadcast_error(&reason).await;
                    break;
                }
                Err(err) => {
                    self.broadcast_error(&err.to_string()).await;
                    break;
                }
            }
        }
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    async fn broadcast_closed(&self) {
        let senders: Vec<mpsc::Sender<P2pTransportEvent>> =
            self.inbound.lock().values().cloned().collect();
        for mut tx in senders {
            let _ = tx.send(P2pTransportEvent::Closed).await;
        }
    }

    async fn broadcast_error(&self, reason: &str) {
        let senders: Vec<mpsc::Sender<P2pTransportEvent>> =
            self.inbound.lock().values().cloned().collect();
        for mut tx in senders {
            let _ = tx.send(P2pTransportEvent::Error(reason.to_string())).await;
        }
    }

    fn detach(&self, key: &PeerKey) {
        self.inbound.lock().remove(key);
    }

    async fn send_outbound(&self, message: P2pMessage) -> Result<(), P2pTransportError> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(P2pTransportError::Closed);
        }
        self.transport.send(message).await
    }

    /// Close the hub. Idempotent.
    pub async fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        self.transport.close().await;
        self.broadcast_closed().await;
        self.inbound.lock().clear();
    }
}

/// Per-peer transport adapter that satisfies `P2pTransport` while
/// routing through a shared [`KeeperHub`]. Tests and production code
/// pass this directly to `run_bridge`.
pub struct HubBackedTransport<T: P2pTransport + 'static> {
    hub: Arc<KeeperHub<T>>,
    key: PeerKey,
    inbound: AsyncMutex<mpsc::Receiver<P2pTransportEvent>>,
    detached: std::sync::atomic::AtomicBool,
}

impl<T: P2pTransport + 'static> std::fmt::Debug for HubBackedTransport<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubBackedTransport")
            .field("key", &self.key)
            .field(
                "detached",
                &self.detached.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

#[async_trait]
impl<T: P2pTransport + 'static> P2pTransport for HubBackedTransport<T> {
    async fn send(&self, message: P2pMessage) -> Result<(), P2pTransportError> {
        if self.detached.load(std::sync::atomic::Ordering::Acquire) {
            return Err(P2pTransportError::Closed);
        }
        self.hub.send_outbound(message).await
    }

    async fn recv(&self) -> Result<P2pTransportEvent, P2pTransportError> {
        let mut guard = self.inbound.lock().await;
        match guard.next().await {
            Some(event) => Ok(event),
            None => Ok(P2pTransportEvent::Closed),
        }
    }

    async fn close(&self) {
        if !self
            .detached
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            self.hub.detach(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::message::P2pStreamTuple;
    use crate::p2p::transport::InMemoryTransport;

    fn header(room: &str, peer: &str, transport: &str) -> P2pMessageHeader {
        P2pMessageHeader {
            room_id: Some(room.into()),
            peer_id: Some(peer.into()),
            transport_id: Some(transport.into()),
        }
    }

    fn msg_check_in(room: &str, peer: &str, transport: &str) -> P2pMessage {
        P2pMessage::CheckIn {
            header: header(room, peer, transport),
            direction: crate::p2p::message::P2pDirection::Pull,
            stream: P2pStreamTuple {
                vhost: "v".into(),
                app: "a".into(),
                stream: "s".into(),
            },
            sdp: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hub_dispatches_messages_to_correct_peer() {
        let (local, remote) = InMemoryTransport::pair(8);
        let hub = KeeperHub::new(local, KeeperHubConfig::default());
        let key_a = PeerKey::new("room1", "peer-a", "tr-a");
        let key_b = PeerKey::new("room1", "peer-b", "tr-b");
        let bridge_a = hub.attach(key_a.clone()).unwrap();
        let bridge_b = hub.attach(key_b.clone()).unwrap();

        let cancel = CancellationToken::new();
        let cancel_for_dispatch = cancel.clone();
        let hub_for_dispatch = hub.clone();
        let dispatcher = tokio::spawn(async move {
            hub_for_dispatch.run_dispatcher(cancel_for_dispatch).await;
        });

        // Remote sends two messages; each must land on the right
        // bridge inbound channel.
        remote
            .send(msg_check_in("room1", "peer-a", "tr-a"))
            .await
            .unwrap();
        remote
            .send(msg_check_in("room1", "peer-b", "tr-b"))
            .await
            .unwrap();

        match bridge_a.recv().await.unwrap() {
            P2pTransportEvent::Message(P2pMessage::CheckIn { header, .. }) => {
                assert_eq!(header.peer_id.as_deref(), Some("peer-a"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        match bridge_b.recv().await.unwrap() {
            P2pTransportEvent::Message(P2pMessage::CheckIn { header, .. }) => {
                assert_eq!(header.peer_id.as_deref(), Some("peer-b"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        cancel.cancel();
        dispatcher.await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hub_drops_messages_with_unknown_peer() {
        let (local, remote) = InMemoryTransport::pair(4);
        let hub = KeeperHub::new(local, KeeperHubConfig::default());
        let key = PeerKey::new("room1", "peer-known", "tr-known");
        let bridge = hub.attach(key).unwrap();

        let cancel = CancellationToken::new();
        let cancel_for_dispatch = cancel.clone();
        let hub_for_dispatch = hub.clone();
        let dispatcher = tokio::spawn(async move {
            hub_for_dispatch.run_dispatcher(cancel_for_dispatch).await;
        });

        // Send a message with an unknown peer key; the hub should
        // drop it silently. Then send a real one to confirm the
        // dispatcher is still alive.
        remote
            .send(msg_check_in("room1", "peer-stranger", "tr-stranger"))
            .await
            .unwrap();
        remote
            .send(msg_check_in("room1", "peer-known", "tr-known"))
            .await
            .unwrap();

        match bridge.recv().await.unwrap() {
            P2pTransportEvent::Message(P2pMessage::CheckIn { header, .. }) => {
                assert_eq!(header.peer_id.as_deref(), Some("peer-known"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        cancel.cancel();
        dispatcher.await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hub_close_propagates_to_all_attached_bridges() {
        let (local, remote) = InMemoryTransport::pair(4);
        let hub = KeeperHub::new(local, KeeperHubConfig::default());
        let bridge_a = hub.attach(PeerKey::new("r", "a", "ta")).unwrap();
        let bridge_b = hub.attach(PeerKey::new("r", "b", "tb")).unwrap();

        let cancel = CancellationToken::new();
        let cancel_for_dispatch = cancel.clone();
        let hub_for_dispatch = hub.clone();
        let dispatcher = tokio::spawn(async move {
            hub_for_dispatch.run_dispatcher(cancel_for_dispatch).await;
        });

        // Close the remote so the hub sees `Closed`.
        remote.close().await;

        match bridge_a.recv().await.unwrap() {
            P2pTransportEvent::Closed => {}
            other => panic!("expected Closed for bridge_a: {other:?}"),
        }
        match bridge_b.recv().await.unwrap() {
            P2pTransportEvent::Closed => {}
            other => panic!("expected Closed for bridge_b: {other:?}"),
        }

        dispatcher.await.unwrap();
        cancel.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hub_rejects_duplicate_peer_attach() {
        let (local, _remote) = InMemoryTransport::pair(4);
        let hub = KeeperHub::new(local, KeeperHubConfig::default());
        let key = PeerKey::new("r", "p", "t");
        let _first = hub.attach(key.clone()).unwrap();
        let err = hub.attach(key.clone()).unwrap_err();
        assert_eq!(err, KeeperHubError::DuplicateKey(key));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hub_max_peers_bounds_attach() {
        let (local, _remote) = InMemoryTransport::pair(4);
        let hub = KeeperHub::new(
            local,
            KeeperHubConfig {
                max_peers: 2,
                ..Default::default()
            },
        );
        let _a = hub.attach(PeerKey::new("r", "a", "ta")).unwrap();
        let _b = hub.attach(PeerKey::new("r", "b", "tb")).unwrap();
        let err = hub.attach(PeerKey::new("r", "c", "tc")).unwrap_err();
        assert_eq!(err, KeeperHubError::CapacityExceeded(2));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_send_through_hub_reaches_remote() {
        let (local, remote) = InMemoryTransport::pair(4);
        let hub = KeeperHub::new(local, KeeperHubConfig::default());
        let bridge = hub.attach(PeerKey::new("r", "p", "t")).unwrap();

        let cancel = CancellationToken::new();
        let cancel_for_dispatch = cancel.clone();
        let hub_for_dispatch = hub.clone();
        let dispatcher = tokio::spawn(async move {
            hub_for_dispatch.run_dispatcher(cancel_for_dispatch).await;
        });

        bridge
            .send(P2pMessage::Ping {
                header: header("r", "p", "t"),
            })
            .await
            .unwrap();

        match remote.recv().await.unwrap() {
            P2pTransportEvent::Message(P2pMessage::Ping { header }) => {
                assert_eq!(header.peer_id.as_deref(), Some("p"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        cancel.cancel();
        dispatcher.await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bridge_close_detaches_from_hub() {
        let (local, _remote) = InMemoryTransport::pair(4);
        let hub = KeeperHub::new(local, KeeperHubConfig::default());
        let key = PeerKey::new("r", "p", "t");
        let bridge = hub.attach(key.clone()).unwrap();
        assert_eq!(hub.peer_count(), 1);
        bridge.close().await;
        assert_eq!(hub.peer_count(), 0);
        // After close, attaching the same key works again.
        let _again = hub.attach(key).unwrap();
        assert_eq!(hub.peer_count(), 1);
    }
}

#[cfg(test)]
mod end_to_end_tests {
    //! Hub ↔ run_bridge fan-out integration: one signaling channel,
    //! two concurrent peers, each driven by its own `run_bridge`.

    use super::*;
    use crate::p2p::bridge::{
        run_bridge, P2pBridgeConfig, P2pBridgeOutcome, RecordingDriverSink, StaticOfferWaiter,
    };
    use crate::p2p::job::{P2pJobConfig, P2pJobKind, P2pJobState};
    use crate::p2p::message::{P2pMessage, P2pMessageHeader, P2pStreamTuple};
    use crate::p2p::transport::{InMemoryTransport, P2pTransport, P2pTransportEvent};
    use cheetah_runtime_api::CancellationToken;
    use cheetah_webrtc_core::WebRtcSessionId;
    use std::sync::Arc;
    use std::time::Duration;

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

    #[tokio::test(flavor = "current_thread")]
    async fn two_concurrent_bridges_share_a_single_hub() {
        // Build the hub on top of a single `InMemoryTransport`. The
        // remote side of the pair plays the role of the signaling
        // server: it answers both check-ins, sends a `bye` per peer,
        // and exits — bridges shut down after seeing the bye.
        let (local, remote) = InMemoryTransport::pair(16);
        let hub = KeeperHub::new(local, KeeperHubConfig::default());

        let key_a = PeerKey::new("room1", "peer-a", "tr-a");
        let key_b = PeerKey::new("room1", "peer-b", "tr-b");
        let bridge_a = hub.attach(key_a.clone()).unwrap();
        let bridge_b = hub.attach(key_b.clone()).unwrap();

        let cancel_dispatch = CancellationToken::new();
        let cancel_for_dispatch = cancel_dispatch.clone();
        let hub_for_dispatch = hub.clone();
        let dispatcher = tokio::spawn(async move {
            hub_for_dispatch.run_dispatcher(cancel_for_dispatch).await;
        });

        // Server: drain inbound messages until two check-ins have
        // been answered, then return. Outbound bye frames from the
        // bridges are simply discarded — the hub still routes them
        // but the server doesn't need to acknowledge anything.
        let server = tokio::spawn(async move {
            let mut answered: usize = 0;
            loop {
                match remote.recv().await {
                    Ok(P2pTransportEvent::Message(P2pMessage::CheckIn { header, .. })) => {
                        let echo_header = P2pMessageHeader {
                            room_id: header.room_id.clone(),
                            peer_id: header.peer_id.clone(),
                            transport_id: header.transport_id.clone(),
                        };
                        remote
                            .send(P2pMessage::Answer {
                                header: echo_header.clone(),
                                sdp: format!(
                                    "v=0\r\nanswer-{}",
                                    header.peer_id.clone().unwrap_or_default()
                                ),
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
                        answered += 1;
                        if answered >= 2 {
                            return;
                        }
                    }
                    Ok(P2pTransportEvent::Message(_)) => {
                        // Outbound bye / candidate / etc. — ignore.
                        continue;
                    }
                    Ok(P2pTransportEvent::Closed) | Ok(P2pTransportEvent::Error(_)) | Err(_) => {
                        return;
                    }
                }
            }
        });

        // Run two bridges in parallel.
        let driver_a = Arc::new(RecordingDriverSink::default());
        let driver_b = Arc::new(RecordingDriverSink::default());
        let waiter = Arc::new(StaticOfferWaiter {
            sdp: "v=0\r\noffer".into(),
        });
        let cancel_a = CancellationToken::new();
        let cancel_b = CancellationToken::new();

        let cfg_a = P2pBridgeConfig {
            job: job_cfg(P2pJobKind::Pull, "room1", "peer-a", "tr-a"),
            session_id: WebRtcSessionId::new(1001),
            offer_timeout: Duration::from_millis(500),
        };
        let cfg_b = P2pBridgeConfig {
            job: job_cfg(P2pJobKind::Push, "room1", "peer-b", "tr-b"),
            session_id: WebRtcSessionId::new(1002),
            offer_timeout: Duration::from_millis(500),
        };

        let waiter_a = waiter.clone();
        let waiter_b = waiter.clone();
        let bridge_a_task =
            tokio::spawn(
                async move { run_bridge(cfg_a, bridge_a, driver_a, waiter_a, cancel_a).await },
            );
        let bridge_b_task =
            tokio::spawn(
                async move { run_bridge(cfg_b, bridge_b, driver_b, waiter_b, cancel_b).await },
            );

        let outcome_a = bridge_a_task.await.unwrap();
        let outcome_b = bridge_b_task.await.unwrap();
        match outcome_a {
            P2pBridgeOutcome::Completed { final_state } => {
                assert_eq!(final_state, P2pJobState::Bye);
            }
            other => panic!("bridge_a unexpected: {other:?}"),
        }
        match outcome_b {
            P2pBridgeOutcome::Completed { final_state } => {
                assert_eq!(final_state, P2pJobState::Bye);
            }
            other => panic!("bridge_b unexpected: {other:?}"),
        }

        cancel_dispatch.cancel();
        dispatcher.await.unwrap();
        server.await.unwrap();

        // Both peers detached from the hub on shutdown.
        assert_eq!(hub.peer_count(), 0);
    }
}
