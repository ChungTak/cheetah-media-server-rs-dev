//! Keeper supervisor task — drives one [`P2pRoomKeeperRegistry`] entry
//! against a transport factory.
//!
//! Phase 05 follow-up: the supervisor closes the loop between the
//! `P2pRoomKeeperRegistry` and an actual signaling transport. It owns
//! the reconnect / state-update lifecycle:
//!
//! ```text
//! Pending → Connecting → Registered → Reconnecting → ... → Stopped
//!                                  ↘
//!                                   Failed (retry exhausted)
//! ```
//!
//! Each round it asks a `KeeperTransportFactory` for a fresh
//! transport. If `connect` fails the supervisor backs off
//! (`retry_initial_backoff` doubling up to `retry_max_backoff`) and
//! tries again until the parent `CancellationToken` fires.
//!
//! The supervisor is generic over the transport so production code
//! can plug in a real `tokio-tungstenite` factory while tests reuse
//! [`super::transport::InMemoryTransport`]. The transport itself is
//! responsible for any I/O; the supervisor only sequences `connect`
//! calls and registry state transitions.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cheetah_codec::MonoTime;
use cheetah_runtime_api::{CancellationToken, RuntimeApi};
use futures::FutureExt;

use super::hub::{KeeperHub, KeeperHubConfig};
use super::room::{
    P2pKeeperState, P2pKeeperStatus, P2pRoomKeeperKey, P2pRoomKeeperRegistry, P2pRoomKeeperSnapshot,
};
use super::transport::{P2pTransport, P2pTransportError, P2pTransportEvent};

/// Transport factory the supervisor calls every reconnect round.
///
/// Implementations build a fresh `P2pTransport` from the supplied
/// keeper snapshot. Failures are surfaced as `Err`; the supervisor
/// counts them and applies the configured backoff.
#[async_trait]
pub trait KeeperTransportFactory: Send + Sync {
    type Transport: P2pTransport + 'static;

    async fn connect(
        &self,
        snapshot: &P2pRoomKeeperSnapshot,
    ) -> Result<Self::Transport, P2pTransportError>;
}

/// Reconnect / retry knobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeeperSupervisorConfig {
    /// `retry_initial_backoff` field of type `Duration`.
    /// `retry_initial_backoff` 字段，类型为 `Duration`.
    pub retry_initial_backoff: Duration,
    /// `retry_max_backoff` field of type `Duration`.
    /// `retry_max_backoff` 字段，类型为 `Duration`.
    pub retry_max_backoff: Duration,
    /// Maximum reconnect attempts before giving up. `0` means
    /// "retry forever until cancelled".
    pub max_attempts: u32,
}

impl Default for KeeperSupervisorConfig {
    fn default() -> Self {
        Self {
            retry_initial_backoff: Duration::from_millis(500),
            retry_max_backoff: Duration::from_secs(30),
            max_attempts: 0,
        }
    }
}

/// Outcome of a single supervisor run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeeperSupervisorOutcome {
    /// Cancellation token fired or registry removed the keeper.
    Stopped,
    /// `max_attempts` reached without success.
    GaveUp { last_error: String },
    /// Keeper key vanished from the registry mid-run.
    KeeperRemoved,
}

/// Run the supervisor loop. Returns when the parent cancel fires, the
/// keeper is removed, or `max_attempts` is reached.
pub async fn run_supervisor<F>(
    registry: Arc<P2pRoomKeeperRegistry>,
    key: P2pRoomKeeperKey,
    config: KeeperSupervisorConfig,
    factory: F,
    runtime: Arc<dyn RuntimeApi>,
    cancel: CancellationToken,
) -> KeeperSupervisorOutcome
where
    F: KeeperTransportFactory,
{
    let mut backoff = config.retry_initial_backoff.max(Duration::from_millis(50));
    let mut attempts: u32 = 0;
    let mut last_error: Option<String> = None;

    loop {
        if cancel.is_cancelled() {
            update_status(
                &registry,
                key,
                P2pKeeperState::Stopped,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::Stopped;
        }
        let snapshot = match find_snapshot(&registry, key) {
            Some(s) => s,
            None => return KeeperSupervisorOutcome::KeeperRemoved,
        };

        // Connect.
        update_status(
            &registry,
            key,
            P2pKeeperState::Connecting,
            last_error.clone(),
            attempts,
        );
        let transport = match factory.connect(&snapshot).await {
            Ok(t) => t,
            Err(err) => {
                attempts = attempts.saturating_add(1);
                last_error = Some(err.to_string());
                if config.max_attempts != 0 && attempts >= config.max_attempts {
                    update_status(
                        &registry,
                        key,
                        P2pKeeperState::Failed,
                        last_error.clone(),
                        attempts,
                    );
                    return KeeperSupervisorOutcome::GaveUp {
                        last_error: last_error.unwrap_or_default(),
                    };
                }
                update_status(
                    &registry,
                    key,
                    P2pKeeperState::Reconnecting,
                    last_error.clone(),
                    attempts,
                );
                if !sleep_with_cancel(backoff, &cancel, &runtime).await {
                    update_status(
                        &registry,
                        key,
                        P2pKeeperState::Stopped,
                        last_error.clone(),
                        attempts,
                    );
                    return KeeperSupervisorOutcome::Stopped;
                }
                backoff = (backoff * 2).min(config.retry_max_backoff);
                continue;
            }
        };

        // Connected. Reset backoff so the next disconnect starts fresh.
        backoff = config.retry_initial_backoff.max(Duration::from_millis(50));
        update_status(&registry, key, P2pKeeperState::Registered, None, attempts);

        let disconnect_reason = pump_until_disconnect(&transport, &cancel).await;
        transport.close().await;

        if cancel.is_cancelled() {
            update_status(
                &registry,
                key,
                P2pKeeperState::Stopped,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::Stopped;
        }
        if find_snapshot(&registry, key).is_none() {
            return KeeperSupervisorOutcome::KeeperRemoved;
        }
        last_error = Some(disconnect_reason);
        attempts = attempts.saturating_add(1);
        if config.max_attempts != 0 && attempts >= config.max_attempts {
            update_status(
                &registry,
                key,
                P2pKeeperState::Failed,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::GaveUp {
                last_error: last_error.unwrap_or_default(),
            };
        }
        update_status(
            &registry,
            key,
            P2pKeeperState::Reconnecting,
            last_error.clone(),
            attempts,
        );
        if !sleep_with_cancel(backoff, &cancel, &runtime).await {
            update_status(
                &registry,
                key,
                P2pKeeperState::Stopped,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::Stopped;
        }
        backoff = (backoff * 2).min(config.retry_max_backoff);
    }
}

async fn pump_until_disconnect<T: P2pTransport>(
    transport: &T,
    cancel: &CancellationToken,
) -> String {
    loop {
        let event = {
            let cancelled = cancel.cancelled().fuse();
            let recv = transport.recv().fuse();
            futures::pin_mut!(cancelled, recv);
            futures::select_biased! {
                _ = cancelled => return "supervisor cancelled".into(),
                res = recv => res,
            }
        };
        match event {
            Ok(P2pTransportEvent::Closed) => return "transport closed".into(),
            Ok(P2pTransportEvent::Error(reason)) => return reason,
            Ok(P2pTransportEvent::Message(_)) => {
                // Today the supervisor is purely transport-level; the
                // schema-aware P2P session pump lives in
                // `super::bridge::run_bridge`. Inbound messages are
                // ignored here so the supervisor doesn't compete for
                // them. A future round will fan messages out.
                continue;
            }
            Err(err) => return err.to_string(),
        }
    }
}

async fn sleep_with_cancel(
    dur: Duration,
    cancel: &CancellationToken,
    runtime: &Arc<dyn RuntimeApi>,
) -> bool {
    let dur_us = u64::try_from(dur.as_micros()).unwrap_or(u64::MAX);
    let deadline = MonoTime::from_micros(runtime.now().as_micros().saturating_add(dur_us));
    let mut timer = runtime.sleep_until(deadline);
    let cancelled = cancel.cancelled().fuse();
    let wait = timer.wait().fuse();
    futures::pin_mut!(cancelled, wait);
    futures::select_biased! {
        _ = cancelled => false,
        _ = wait => true,
    }
}

fn find_snapshot(
    registry: &Arc<P2pRoomKeeperRegistry>,
    key: P2pRoomKeeperKey,
) -> Option<P2pRoomKeeperSnapshot> {
    registry.list().into_iter().find(|s| s.key == key)
}

fn update_status(
    registry: &Arc<P2pRoomKeeperRegistry>,
    key: P2pRoomKeeperKey,
    state: P2pKeeperState,
    last_error: Option<String>,
    reconnect_attempts: u32,
) {
    registry.set_status(
        key,
        P2pKeeperStatus {
            state,
            last_error,
            reconnect_attempts,
        },
    );
}

/// Observer notified each time the supervisor brings a hub up.
///
/// Production code uses this hook to attach `run_bridge` calls onto
/// the freshly registered hub. The observer's `on_hub_ready` is
/// awaited once per successful connect; the supervisor then runs the
/// hub's dispatcher until either the parent cancel fires, the hub
/// reports the transport closing, or `on_hub_ready` itself returns.
///
/// The observer **owns** the bridges spawned on top of the hub and is
/// responsible for cancelling them when the hub goes away. The
/// supervisor only signals the lifetime via the per-hub
/// [`CancellationToken`] passed to `on_hub_ready`.
#[async_trait]
pub trait KeeperHubObserver: Send + Sync {
    type Transport: P2pTransport + 'static;

    /// Called every time the supervisor establishes a fresh transport
    /// and wraps it in a [`KeeperHub`]. The hub's dispatcher runs in
    /// parallel with `on_hub_ready`; the observer can attach peer
    /// bridges via [`KeeperHub::attach`].
    ///
    /// `hub_cancel` fires when the supervisor is about to tear the
    /// hub down (transport closed, retry exhausted, parent
    /// cancellation). Observers must release any bridge tasks they
    /// own when this token fires.
    async fn on_hub_ready(
        &self,
        snapshot: P2pRoomKeeperSnapshot,
        hub: Arc<KeeperHub<Self::Transport>>,
        hub_cancel: CancellationToken,
    );
}

/// Run the supervisor and wrap each connected transport in a
/// [`KeeperHub`]. Mirrors [`run_supervisor`] but adds the per-connect
/// `on_hub_ready` callback so callers can attach bridges.
///
/// The hub's dispatcher and the observer's `on_hub_ready` future run
/// concurrently; the supervisor returns control to its main loop
/// (state-machine + reconnect) only after the dispatcher exits, the
/// hub closes, or the observer drops the cancellation. Concretely:
///
/// 1. `factory.connect` brings up a transport.
/// 2. The transport is wrapped in `KeeperHub` and the observer is
///    spawned in the background.
/// 3. The hub dispatcher pumps inbound messages until the transport
///    closes / errors / the parent cancel fires.
/// 4. Once the dispatcher exits, the per-hub cancel fires; the
///    observer's `on_hub_ready` future is awaited so its bridges can
///    finish their teardown.
/// 5. Standard reconnect / give-up logic kicks in.
#[allow(clippy::too_many_arguments)]
pub async fn run_supervisor_with_hub<F, O>(
    registry: Arc<P2pRoomKeeperRegistry>,
    key: P2pRoomKeeperKey,
    config: KeeperSupervisorConfig,
    hub_config: KeeperHubConfig,
    factory: F,
    observer: Arc<O>,
    runtime: Arc<dyn RuntimeApi>,
    cancel: CancellationToken,
) -> KeeperSupervisorOutcome
where
    F: KeeperTransportFactory,
    O: KeeperHubObserver<Transport = F::Transport> + 'static,
{
    let mut backoff = config.retry_initial_backoff.max(Duration::from_millis(50));
    let mut attempts: u32 = 0;
    let mut last_error: Option<String> = None;

    loop {
        if cancel.is_cancelled() {
            update_status(
                &registry,
                key,
                P2pKeeperState::Stopped,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::Stopped;
        }
        let snapshot = match find_snapshot(&registry, key) {
            Some(s) => s,
            None => return KeeperSupervisorOutcome::KeeperRemoved,
        };

        update_status(
            &registry,
            key,
            P2pKeeperState::Connecting,
            last_error.clone(),
            attempts,
        );
        let transport = match factory.connect(&snapshot).await {
            Ok(t) => t,
            Err(err) => {
                attempts = attempts.saturating_add(1);
                last_error = Some(err.to_string());
                if config.max_attempts != 0 && attempts >= config.max_attempts {
                    update_status(
                        &registry,
                        key,
                        P2pKeeperState::Failed,
                        last_error.clone(),
                        attempts,
                    );
                    return KeeperSupervisorOutcome::GaveUp {
                        last_error: last_error.unwrap_or_default(),
                    };
                }
                update_status(
                    &registry,
                    key,
                    P2pKeeperState::Reconnecting,
                    last_error.clone(),
                    attempts,
                );
                if !sleep_with_cancel(backoff, &cancel, &runtime).await {
                    update_status(
                        &registry,
                        key,
                        P2pKeeperState::Stopped,
                        last_error.clone(),
                        attempts,
                    );
                    return KeeperSupervisorOutcome::Stopped;
                }
                backoff = (backoff * 2).min(config.retry_max_backoff);
                continue;
            }
        };

        backoff = config.retry_initial_backoff.max(Duration::from_millis(50));
        update_status(&registry, key, P2pKeeperState::Registered, None, attempts);

        // Wrap the transport in a hub, hand it to the observer, and
        // pump the dispatcher until something tears the connection
        // down.
        let hub = KeeperHub::new(transport, hub_config.clone());
        let hub_cancel = cancel.child_token();
        let observer_handle = {
            let observer = observer.clone();
            let snapshot_for_observer = snapshot.clone();
            let hub_for_observer = hub.clone();
            let hub_cancel_for_observer = hub_cancel.clone();
            runtime.spawn(Box::pin(async move {
                observer
                    .on_hub_ready(
                        snapshot_for_observer,
                        hub_for_observer,
                        hub_cancel_for_observer,
                    )
                    .await;
            }))
        };

        // Watchdog: if the keeper is removed from the registry
        // mid-connection, the dispatcher would otherwise sit on the
        // transport recv forever. We poll the registry and cancel
        // the hub on removal so the supervisor can re-evaluate.
        let watchdog_handle = {
            let registry = registry.clone();
            let hub_cancel = hub_cancel.clone();
            let watchdog_runtime = runtime.clone();
            runtime.spawn(Box::pin(async move {
                loop {
                    if hub_cancel.is_cancelled() {
                        return;
                    }
                    if find_snapshot(&registry, key).is_none() {
                        hub_cancel.cancel();
                        return;
                    }
                    let poll_us = Duration::from_millis(100).as_micros() as u64;
                    let deadline = MonoTime::from_micros(
                        watchdog_runtime.now().as_micros().saturating_add(poll_us),
                    );
                    let mut timer = watchdog_runtime.sleep_until(deadline);
                    let cancelled = hub_cancel.cancelled().fuse();
                    let wait = timer.wait().fuse();
                    futures::pin_mut!(cancelled, wait);
                    futures::select_biased! {
                        _ = cancelled => return,
                        _ = wait => {}
                    }
                }
            }))
        };

        let dispatch_cancel = hub_cancel.clone();
        hub.run_dispatcher(dispatch_cancel).await;
        // Ask the observer to wind down its bridges.
        hub_cancel.cancel();
        // Wait for the observer to clean up so subsequent reconnect
        // rounds do not race against half-detached bridges.
        let _ = observer_handle.wait().await;
        let _ = watchdog_handle.wait().await;
        hub.close().await;

        if cancel.is_cancelled() {
            update_status(
                &registry,
                key,
                P2pKeeperState::Stopped,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::Stopped;
        }
        if find_snapshot(&registry, key).is_none() {
            return KeeperSupervisorOutcome::KeeperRemoved;
        }
        last_error = Some("hub disconnected".into());
        attempts = attempts.saturating_add(1);
        if config.max_attempts != 0 && attempts >= config.max_attempts {
            update_status(
                &registry,
                key,
                P2pKeeperState::Failed,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::GaveUp {
                last_error: last_error.unwrap_or_default(),
            };
        }
        update_status(
            &registry,
            key,
            P2pKeeperState::Reconnecting,
            last_error.clone(),
            attempts,
        );
        if !sleep_with_cancel(backoff, &cancel, &runtime).await {
            update_status(
                &registry,
                key,
                P2pKeeperState::Stopped,
                last_error.clone(),
                attempts,
            );
            return KeeperSupervisorOutcome::Stopped;
        }
        backoff = (backoff * 2).min(config.retry_max_backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::room::P2pRoomKeeperConfig;
    use crate::p2p::transport::InMemoryTransport;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_runtime() -> Arc<dyn RuntimeApi> {
        Arc::new(cheetah_runtime_tokio::TokioRuntime::new())
    }

    fn cfg(room: &str) -> P2pRoomKeeperConfig {
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

    /// Factory that returns a fresh `InMemoryTransport` pair per
    /// round, parking the peer side so `recv` blocks until the test
    /// closes it.
    struct PairFactory {
        connects: Arc<AtomicUsize>,
        peers: Arc<Mutex<Vec<InMemoryTransport>>>,
    }

    #[async_trait]
    impl KeeperTransportFactory for PairFactory {
        type Transport = InMemoryTransport;
        async fn connect(
            &self,
            _snapshot: &P2pRoomKeeperSnapshot,
        ) -> Result<Self::Transport, P2pTransportError> {
            self.connects.fetch_add(1, Ordering::Relaxed);
            let (local, remote) = InMemoryTransport::pair(4);
            self.peers.lock().push(remote);
            Ok(local)
        }
    }

    /// Factory that always fails. Used to verify backoff + give-up.
    struct AlwaysFailFactory {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl KeeperTransportFactory for AlwaysFailFactory {
        type Transport = InMemoryTransport;
        async fn connect(
            &self,
            _snapshot: &P2pRoomKeeperSnapshot,
        ) -> Result<Self::Transport, P2pTransportError> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Err(P2pTransportError::Io("connect refused".into()))
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn supervisor_marks_registered_on_first_connect_then_reconnects_on_disconnect() {
        let registry = Arc::new(P2pRoomKeeperRegistry::default());
        let key = registry.add(cfg("room42")).unwrap();
        let connects = Arc::new(AtomicUsize::new(0));
        let peers = Arc::new(Mutex::new(Vec::new()));
        let factory = PairFactory {
            connects: connects.clone(),
            peers: peers.clone(),
        };
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let registry_for_task = registry.clone();
        let task = tokio::spawn(async move {
            run_supervisor(
                registry_for_task,
                key,
                KeeperSupervisorConfig {
                    retry_initial_backoff: Duration::from_millis(50),
                    retry_max_backoff: Duration::from_millis(200),
                    max_attempts: 3,
                },
                factory,
                test_runtime(),
                cancel_for_task,
            )
            .await
        });

        // Wait until the first connect lands and the keeper is
        // marked Registered. Tokio's paused clock means we have to
        // yield a few times to let the supervisor make progress.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if connects.load(Ordering::Relaxed) >= 1 {
                break;
            }
        }
        for _ in 0..50 {
            tokio::task::yield_now().await;
            let snap = registry.list().into_iter().find(|s| s.key == key).unwrap();
            if snap.status.state == P2pKeeperState::Registered {
                break;
            }
        }
        let snap = registry.list().into_iter().find(|s| s.key == key).unwrap();
        assert_eq!(snap.status.state, P2pKeeperState::Registered);

        // Drop the peer side to simulate a disconnect.
        {
            let mut guard = peers.lock();
            guard.clear();
        }

        // The supervisor should bump connects again as it reconnects.
        for _ in 0..200 {
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_millis(60)).await;
            if connects.load(Ordering::Relaxed) >= 2 {
                break;
            }
        }
        assert!(
            connects.load(Ordering::Relaxed) >= 2,
            "supervisor should reconnect after disconnect (connects={})",
            connects.load(Ordering::Relaxed)
        );

        cancel.cancel();
        let outcome = task.await.unwrap();
        match outcome {
            KeeperSupervisorOutcome::Stopped | KeeperSupervisorOutcome::GaveUp { .. } => {}
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn supervisor_gives_up_after_max_attempts() {
        let registry = Arc::new(P2pRoomKeeperRegistry::default());
        let key = registry.add(cfg("room42")).unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let factory = AlwaysFailFactory {
            attempts: attempts.clone(),
        };
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let registry_for_task = registry.clone();
        let task = tokio::spawn(async move {
            run_supervisor(
                registry_for_task,
                key,
                KeeperSupervisorConfig {
                    retry_initial_backoff: Duration::from_millis(50),
                    retry_max_backoff: Duration::from_millis(200),
                    max_attempts: 3,
                },
                factory,
                test_runtime(),
                cancel_for_task,
            )
            .await
        });

        // Pump time forward so the supervisor exhausts attempts.
        for _ in 0..50 {
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
            if attempts.load(Ordering::Relaxed) >= 3 {
                break;
            }
        }
        let outcome = task.await.unwrap();
        match outcome {
            KeeperSupervisorOutcome::GaveUp { last_error } => {
                assert!(last_error.contains("connect"), "unexpected: {last_error}");
            }
            other => panic!("expected GaveUp, got {other:?}"),
        }
        let snap = registry.list().into_iter().find(|s| s.key == key).unwrap();
        assert_eq!(snap.status.state, P2pKeeperState::Failed);
        cancel.cancel();
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn supervisor_returns_keeper_removed_when_registry_drops_entry() {
        let registry = Arc::new(P2pRoomKeeperRegistry::default());
        let key = registry.add(cfg("room42")).unwrap();
        let connects = Arc::new(AtomicUsize::new(0));
        let peers = Arc::new(Mutex::new(Vec::new()));
        let factory = PairFactory {
            connects: connects.clone(),
            peers: peers.clone(),
        };
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let registry_for_task = registry.clone();
        let task = tokio::spawn(async move {
            run_supervisor(
                registry_for_task,
                key,
                KeeperSupervisorConfig::default(),
                factory,
                test_runtime(),
                cancel_for_task,
            )
            .await
        });

        // Wait for first connect.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if connects.load(Ordering::Relaxed) >= 1 {
                break;
            }
        }

        // Remove the keeper while the supervisor is connected; then
        // close the peer to force a re-evaluation, which should see
        // an empty registry and exit.
        let _ = registry.remove(key);
        peers.lock().clear();
        for _ in 0..50 {
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
        let outcome = task.await.unwrap();
        assert_eq!(outcome, KeeperSupervisorOutcome::KeeperRemoved);
    }
}
