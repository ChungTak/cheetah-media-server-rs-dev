//! P2P pull/push client job runner.
//!
//! Phase 05 follow-up — round 9: completes the HTTP 200 path for
//! `webrtc://...?signaling_protocols=1` URLs by orchestrating the
//! supervisor + hub + bridge stack against a real
//! [`crate::p2p::WebSocketTransportFactory`].
//!
//! Architecture:
//!
//! ```text
//! HTTP /pull/start (P2P)
//!     │
//!     ▼
//! P2pClientJobRegistry::start
//!     │
//!     ├─▶ register snapshot (Pending)
//!     │
//!     ▼
//! tokio::spawn supervisor:
//!     run_supervisor_with_hub(
//!         registry, key, config, hub_config,
//!         WebSocketTransportFactory,
//!         OneBridgeObserver { run_bridge_with_lifecycle },
//!         cancel,
//!     )
//! ```
//!
//! The supervisor brings up a real WebSocket connection, wraps it in
//! a `KeeperHub`, and the observer runs a single P2P bridge per
//! connection. The job snapshot (Pending → Connecting → Connected →
//! Stopped/Failed) is mirrored into a `parking_lot::Mutex` so the
//! `/list` endpoint can render it.
//!
//! This is the runner the gap-analysis lists as the final blocker
//! for "pull/push HTTP 真正 P2P 客户端 job runner". Once it's wired
//! into `WebRtcHttpService`, P2P URLs return `200` + a `session_id`
//! handle instead of `501`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_runtime_api::CancellationToken;
use cheetah_sdk::EngineContext;
use cheetah_webrtc_core::WebRtcSessionId;
use cheetah_webrtc_driver_tokio::WebRtcDriverHandle;
use parking_lot::Mutex;
use thiserror::Error;

use crate::p2p::{
    bridge::{P2pBridgeConfig, P2pBridgeOutcome},
    entrypoint::{plan_from_zlm_url, P2pBridgePlanInput},
    hub::{KeeperHub, PeerKey},
    job::P2pJobKind,
    room::{P2pRoomKeeperConfig, P2pRoomKeeperKey, P2pRoomKeeperRegistry, P2pRoomKeeperSnapshot},
    supervisor::{
        run_supervisor_with_hub, KeeperHubObserver, KeeperSupervisorConfig, KeeperSupervisorOutcome,
    },
    KeeperHubConfig, LifecycleDispatcher, NoopLifecycleSource, SignalingUrlPolicy,
    WebSocketP2pTransport, WebSocketTransportConfig, WebSocketTransportFactory,
};

/// Lifecycle states surfaced to operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pClientJobState {
    /// Job registered but supervisor hasn't yielded any state yet.
    Pending,
    /// Supervisor has connected to the signaling server, bridge is
    /// in `AwaitingAnswer` or `Connected`.
    Running,
    /// Job finished cleanly (bridge reached `Bye`, supervisor returned).
    Stopped,
    /// Job failed (transport or supervisor reported `GaveUp`).
    Failed,
}

/// Snapshot returned by the `/list` endpoint.
#[derive(Debug, Clone)]
pub struct P2pClientJobSnapshot {
    pub session_id: WebRtcSessionId,
    pub kind: P2pJobKind,
    pub url: String,
    pub state: P2pClientJobState,
    pub last_error: Option<String>,
    pub signaling_url: String,
    pub peer_room_id: String,
    pub stream_key: String,
}

#[derive(Debug, Error)]
pub enum P2pClientJobError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("job already running for {0}")]
    Conflict(String),
    #[error("driver unavailable")]
    DriverUnavailable,
}

/// Configuration for [`P2pClientJobRegistry::start`]. Exposed so the
/// HTTP layer can map JSON body fields onto it without leaking
/// transport types.
#[derive(Debug, Clone)]
pub struct P2pClientJobRequest {
    pub url: String,
    pub kind: P2pJobKind,
    pub allow_private_ips: bool,
    /// Override the entire signaling URL. When `None`, the registry
    /// derives `ws(s)://host:port/index/api/webrtc` from the URL.
    pub signaling_url_override: Option<String>,
    /// Connect timeout for the WebSocket handshake.
    pub connect_timeout: Duration,
    /// Per-bridge offer timeout. The bridge waits this long for the
    /// driver to produce an SDP after `CreateOffer`.
    pub offer_timeout: Duration,
    /// Per-job retry knobs. Defaults are conservative.
    pub supervisor: KeeperSupervisorConfig,
}

impl Default for P2pClientJobRequest {
    fn default() -> Self {
        Self {
            url: String::new(),
            kind: P2pJobKind::Pull,
            allow_private_ips: false,
            signaling_url_override: None,
            connect_timeout: Duration::from_secs(10),
            offer_timeout: Duration::from_secs(10),
            supervisor: KeeperSupervisorConfig::default(),
        }
    }
}

/// Registry of in-flight P2P client jobs keyed by `session_id`.
///
/// Cheap to clone (single `Arc<Mutex<...>>`). The HTTP service holds
/// one per module instance.
#[derive(Default)]
pub struct P2pClientJobRegistry {
    inner: Mutex<HashMap<WebRtcSessionId, P2pClientJobEntry>>,
}

struct P2pClientJobEntry {
    snapshot: Arc<Mutex<P2pClientJobSnapshot>>,
    cancel: CancellationToken,
    keeper_key: P2pRoomKeeperKey,
}

impl P2pClientJobRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn list(&self) -> Vec<P2pClientJobSnapshot> {
        self.inner
            .lock()
            .values()
            .map(|e| e.snapshot.lock().clone())
            .collect()
    }

    pub fn stop(&self, session_id: WebRtcSessionId) -> bool {
        let entry = self.inner.lock().remove(&session_id);
        match entry {
            Some(entry) => {
                entry.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn stop_all(&self) {
        let entries: Vec<_> = self.inner.lock().drain().collect();
        for (_, entry) in entries {
            entry.cancel.cancel();
        }
    }

    fn register(
        &self,
        session_id: WebRtcSessionId,
        snapshot: Arc<Mutex<P2pClientJobSnapshot>>,
        cancel: CancellationToken,
        keeper_key: P2pRoomKeeperKey,
    ) -> Result<(), P2pClientJobError> {
        let mut guard = self.inner.lock();
        if guard.contains_key(&session_id) {
            return Err(P2pClientJobError::Conflict(session_id.to_string()));
        }
        guard.insert(
            session_id,
            P2pClientJobEntry {
                snapshot,
                cancel,
                keeper_key,
            },
        );
        Ok(())
    }

    fn unregister(&self, session_id: WebRtcSessionId) -> Option<P2pRoomKeeperKey> {
        self.inner.lock().remove(&session_id).map(|e| e.keeper_key)
    }
}

/// All the runtime handles a P2P client job needs. Bundled so the
/// HTTP layer can pass a single struct instead of plumbing five
/// independent fields.
///
/// The `answer_dispatcher` field is `pub(crate)` because the
/// `AnswerDispatcher` type is itself crate-private; HTTP-side code
/// constructs the runtime and the spawn callsite stays inside this
/// crate.
pub struct P2pClientJobRuntime {
    pub registry: Arc<P2pClientJobRegistry>,
    pub keepers: Arc<P2pRoomKeeperRegistry>,
    pub driver: Arc<WebRtcDriverHandle>,
    pub lifecycle: Arc<LifecycleDispatcher>,
    pub engine: EngineContext,
    pub parent_cancel: CancellationToken,
    /// `AnswerDispatcher` used to wait for the driver's `OfferReady`
    /// SDP. The bridge subscribes through this dispatcher instead of
    /// the placeholder `InlineOfferWaiter`.
    pub(crate) answer_dispatcher: Arc<crate::http::AnswerDispatcher>,
}

/// Spawn a new P2P client job. Returns the assigned session id and a
/// snapshot describing the registered state. The supervisor task runs
/// in the background until the parent cancel fires or the keeper is
/// removed.
pub fn spawn(
    runtime: P2pClientJobRuntime,
    session_id: WebRtcSessionId,
    request: P2pClientJobRequest,
) -> Result<P2pClientJobSnapshot, P2pClientJobError> {
    // 1. Parse the URL and run it through the standard plan validator.
    let parsed = crate::compat::parse_zlm_rtc_url(&request.url)
        .map_err(|e| P2pClientJobError::InvalidUrl(e.to_string()))?;
    let policy = SignalingUrlPolicy {
        allow_private_ips: request.allow_private_ips,
        ..Default::default()
    };
    let plan = plan_from_zlm_url(P2pBridgePlanInput {
        url: &parsed,
        kind: request.kind,
        session_id,
        local_room_id: format!("ringing_{}", session_id),
        transport_id: format!("tr_{}", session_id),
        policy: &policy,
        pending_candidate_cap: 0,
        offer_timeout: Some(request.offer_timeout),
    })
    .map_err(|e| P2pClientJobError::InvalidUrl(e.to_string()))?;

    // 2. Register a keeper entry. The supervisor uses the keeper as
    //    the source of `(server_host, server_port, ssl)` for the
    //    transport factory.
    let keeper_key = runtime
        .keepers
        .add(P2pRoomKeeperConfig {
            server_host: plan.signaling_url.host.clone(),
            server_port: plan.signaling_url.port,
            room_id: plan.bridge_config.job.peer_room_id.clone(),
            vhost: Some(plan.bridge_config.job.stream.vhost.clone()),
            app: Some(plan.bridge_config.job.stream.app.clone()),
            stream: Some(plan.bridge_config.job.stream.stream.clone()),
            ssl: plan.signaling_url.secure,
        })
        .map_err(|e| P2pClientJobError::InvalidUrl(e.to_string()))?;

    // 3. Build the snapshot + cancel and register them.
    let signaling_url_str = plan.signaling_url.render();
    let stream_key = format!(
        "{}/{}",
        plan.bridge_config.job.stream.app, plan.bridge_config.job.stream.stream
    );
    let snapshot = Arc::new(Mutex::new(P2pClientJobSnapshot {
        session_id,
        kind: plan.kind,
        url: request.url.clone(),
        state: P2pClientJobState::Pending,
        last_error: None,
        signaling_url: signaling_url_str.clone(),
        peer_room_id: plan.bridge_config.job.peer_room_id.clone(),
        stream_key,
    }));
    let cancel = runtime.parent_cancel.child_token();
    runtime
        .registry
        .register(session_id, snapshot.clone(), cancel.clone(), keeper_key)?;

    let initial = snapshot.lock().clone();

    // 4. Spawn the supervisor task. It owns the `WebSocketTransportFactory`
    //    and the `KeeperHubObserver`, and updates the snapshot as the
    //    state machine progresses.
    let runtime_api = runtime.engine.runtime_api.clone();
    let registry_for_task = runtime.registry.clone();
    let keepers_for_task = runtime.keepers.clone();
    let driver_for_task = runtime.driver.clone();
    let lifecycle_for_task = runtime.lifecycle.clone();
    let answer_dispatcher_for_task = runtime.answer_dispatcher.clone();
    let supervisor_config = request.supervisor.clone();
    let bridge_config = plan.bridge_config.clone();
    let signaling_override = request
        .signaling_url_override
        .clone()
        .or(Some(signaling_url_str));
    let policy = policy.clone();
    let connect_timeout = request.connect_timeout;
    let snapshot_for_task = snapshot.clone();

    runtime_api.spawn(Box::pin(async move {
        run_job(
            registry_for_task,
            keepers_for_task,
            keeper_key,
            session_id,
            snapshot_for_task,
            cancel,
            bridge_config,
            driver_for_task,
            lifecycle_for_task,
            answer_dispatcher_for_task,
            policy,
            signaling_override,
            connect_timeout,
            supervisor_config,
        )
        .await;
    }));

    Ok(initial)
}

#[allow(clippy::too_many_arguments)]
async fn run_job(
    registry: Arc<P2pClientJobRegistry>,
    keepers: Arc<P2pRoomKeeperRegistry>,
    keeper_key: P2pRoomKeeperKey,
    session_id: WebRtcSessionId,
    snapshot: Arc<Mutex<P2pClientJobSnapshot>>,
    cancel: CancellationToken,
    bridge_config: P2pBridgeConfig,
    driver: Arc<WebRtcDriverHandle>,
    lifecycle: Arc<LifecycleDispatcher>,
    answer_dispatcher: Arc<crate::http::AnswerDispatcher>,
    policy: SignalingUrlPolicy,
    signaling_override: Option<String>,
    connect_timeout: Duration,
    supervisor_config: KeeperSupervisorConfig,
) {
    let factory = WebSocketTransportFactory::new(WebSocketTransportConfig {
        url_policy: policy,
        decoder: Default::default(),
        connect_timeout,
        url_override: signaling_override,
    });
    let bridge_outcome = Arc::new(Mutex::new(None::<P2pBridgeOutcome>));
    let observer = Arc::new(P2pClientObserver {
        peer_key: PeerKey::new(
            bridge_config.job.peer_room_id.clone(),
            bridge_config.job.local_room_id.clone(),
            bridge_config.job.transport_id.clone(),
        ),
        bridge_config: bridge_config.clone(),
        driver: driver.clone(),
        lifecycle,
        answer_dispatcher: answer_dispatcher.clone(),
        bridge_outcome: bridge_outcome.clone(),
        snapshot: snapshot.clone(),
    });

    {
        let mut guard = snapshot.lock();
        guard.state = P2pClientJobState::Running;
    }

    let outcome = run_supervisor_with_hub(
        keepers.clone(),
        keeper_key,
        supervisor_config,
        KeeperHubConfig::default(),
        factory,
        observer,
        cancel,
    )
    .await;

    // Update the final state based on bridge + supervisor outcome.
    let bridge_final = bridge_outcome.lock().clone();
    let final_state = match (&bridge_final, &outcome) {
        (Some(P2pBridgeOutcome::Completed { .. }), _) => P2pClientJobState::Stopped,
        (Some(P2pBridgeOutcome::OfferFailed { .. }), _)
        | (Some(P2pBridgeOutcome::TransportError { .. }), _)
        | (Some(P2pBridgeOutcome::Encode { .. }), _) => P2pClientJobState::Failed,
        (None, KeeperSupervisorOutcome::KeeperRemoved | KeeperSupervisorOutcome::Stopped) => {
            P2pClientJobState::Stopped
        }
        (None, KeeperSupervisorOutcome::GaveUp { .. }) => P2pClientJobState::Failed,
    };
    {
        let mut guard = snapshot.lock();
        guard.state = final_state;
        if matches!(final_state, P2pClientJobState::Failed) && guard.last_error.is_none() {
            guard.last_error = Some(format!("supervisor outcome: {outcome:?}"));
        }
    }

    // Cleanup: remove keeper + registry entry. Use `unregister` so the
    // entry is gone before the snapshot's `Stopped`/`Failed` is read by
    // the operator.
    let _ = keepers.remove(keeper_key);
    let _ = registry.unregister(session_id);
}

struct P2pClientObserver {
    peer_key: PeerKey,
    bridge_config: P2pBridgeConfig,
    driver: Arc<WebRtcDriverHandle>,
    lifecycle: Arc<LifecycleDispatcher>,
    answer_dispatcher: Arc<crate::http::AnswerDispatcher>,
    bridge_outcome: Arc<Mutex<Option<P2pBridgeOutcome>>>,
    snapshot: Arc<Mutex<P2pClientJobSnapshot>>,
}

#[async_trait]
impl KeeperHubObserver for P2pClientObserver {
    type Transport = WebSocketP2pTransport;

    async fn on_hub_ready(
        &self,
        _snapshot: P2pRoomKeeperSnapshot,
        hub: Arc<KeeperHub<WebSocketP2pTransport>>,
        hub_cancel: CancellationToken,
    ) {
        // Attach the per-peer transport. Failures here are unusual
        // (the hub has just come up and has empty registrations), but
        // we surface the error in the snapshot for the operator.
        let transport = match hub.attach(self.peer_key.clone()) {
            Ok(t) => t,
            Err(err) => {
                self.snapshot.lock().last_error = Some(err.to_string());
                return;
            }
        };

        // Build a `DispatcherOfferWaiter` that subscribes for the
        // driver's local SDP via the module's `AnswerDispatcher`.
        // The dispatcher receives `OfferReady` events from the
        // driver event worker and routes them to the bridge that
        // requested `CreateOffer`. `subscribe_p2p` adapts the
        // crate-private `AnswerOutcome` to the public
        // `DispatcherOfferOutcome` shape the bridge consumes.
        let dispatcher = self.answer_dispatcher.clone();
        let waiter = Arc::new(crate::p2p::DispatcherOfferWaiter::new(move |session_id| {
            dispatcher.subscribe_p2p(session_id)
        }));

        let outcome = crate::p2p::run_bridge_with_lifecycle::<
            _,
            _,
            crate::p2p::DispatcherOfferWaiter,
            LifecycleDispatcher,
        >(
            self.bridge_config.clone(),
            transport,
            self.driver.clone(),
            waiter,
            Some(self.lifecycle.clone()),
            hub_cancel,
        )
        .await;

        *self.bridge_outcome.lock() = Some(outcome);
    }
}

// `NoopLifecycleSource` is re-exported for callers that explicitly
// don't want the dispatcher hook. Keep the import live so the API
// surface stays discoverable.
#[allow(dead_code)]
fn _noop_marker() -> NoopLifecycleSource {
    NoopLifecycleSource
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::P2pRoomKeeperRegistry;

    #[test]
    fn registry_round_trip() {
        let registry = P2pClientJobRegistry::new();
        let session = WebRtcSessionId::new(1);
        let cancel = CancellationToken::new();
        // Synthesize a snapshot directly to test the registry.
        let snap = Arc::new(Mutex::new(P2pClientJobSnapshot {
            session_id: session,
            kind: P2pJobKind::Pull,
            url: "webrtc://example.com/live/demo?signaling_protocols=1&peer_room_id=r".into(),
            state: P2pClientJobState::Pending,
            last_error: None,
            signaling_url: "ws://example.com:80/index/api/webrtc".into(),
            peer_room_id: "r".into(),
            stream_key: "live/demo".into(),
        }));
        let keepers = P2pRoomKeeperRegistry::default();
        let keeper_key = keepers
            .add(P2pRoomKeeperConfig {
                server_host: "example.com".into(),
                server_port: 80,
                room_id: "r".into(),
                vhost: None,
                app: None,
                stream: None,
                ssl: false,
            })
            .unwrap();

        registry
            .register(session, snap.clone(), cancel.clone(), keeper_key)
            .unwrap();
        assert_eq!(registry.list().len(), 1);

        // Conflict.
        let err = registry
            .register(session, snap.clone(), cancel.clone(), keeper_key)
            .unwrap_err();
        assert!(matches!(err, P2pClientJobError::Conflict(_)));

        // Stop returns true and cancels.
        assert!(registry.stop(session));
        assert!(cancel.is_cancelled());
        assert_eq!(registry.list().len(), 0);

        // Stopping a missing entry returns false.
        assert!(!registry.stop(session));
    }

    #[test]
    fn stop_all_drains_registry() {
        let registry = P2pClientJobRegistry::new();
        let keepers = P2pRoomKeeperRegistry::default();
        let cfg = P2pRoomKeeperConfig {
            server_host: "x".into(),
            server_port: 1,
            room_id: "r".into(),
            vhost: None,
            app: None,
            stream: None,
            ssl: false,
        };
        for i in 0..3 {
            let session = WebRtcSessionId::new(100 + i);
            let cancel = CancellationToken::new();
            let snap = Arc::new(Mutex::new(P2pClientJobSnapshot {
                session_id: session,
                kind: P2pJobKind::Pull,
                url: "x".into(),
                state: P2pClientJobState::Pending,
                last_error: None,
                signaling_url: "x".into(),
                peer_room_id: "r".into(),
                stream_key: "x/y".into(),
            }));
            let keeper_key = keepers.add(cfg.clone()).unwrap();
            registry
                .register(session, snap, cancel, keeper_key)
                .unwrap();
        }
        assert_eq!(registry.list().len(), 3);
        registry.stop_all();
        assert_eq!(registry.list().len(), 0);
    }
}
