//! Lifecycle event dispatcher used by the module's driver-event
//! worker to fan `WebRtcCoreEvent::Lifecycle` messages out to the
//! per-session [`BridgeLifecycleSource`] subscribers used by
//! `run_bridge_with_lifecycle`.
//!
//! Phase 05 follow-up: previously the bridge's lifecycle channel was
//! supplied by callers/tests only. This module ties the runtime hook
//! together so production P2P bridges automatically observe the
//! driver's `Connected` / `Closed` transitions.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cheetah_webrtc_core::WebRtcSessionId;
use futures::channel::mpsc;
use parking_lot::Mutex;

use super::bridge::{BridgeLifecycleEvent, BridgeLifecycleSource};

/// Per-process lifecycle dispatcher. Cheap to clone (single
/// `Arc<Mutex<...>>`) so the driver event worker and any number of
/// `run_bridge_with_lifecycle` callers can share it.
#[derive(Debug, Default)]
pub struct LifecycleDispatcher {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    senders: HashMap<WebRtcSessionId, mpsc::Sender<BridgeLifecycleEvent>>,
}

/// Per-session subscription channel size. The bridge only consumes a
/// single `Connected` / `Closed` event per run; capacity 4 leaves
/// headroom for `try_send` to never spill in practice.
const SUBSCRIBE_CAPACITY: usize = 4;

impl LifecycleDispatcher {
    /// Creates a new `LifecycleDispatcher` instance.
    /// 创建新的 `LifecycleDispatcher` 实例。
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Forward a `Connected` lifecycle event. Drops silently if no
    /// bridge has subscribed for this session id (e.g. WHIP/WHEP
    /// sessions that don't use the bridge stack).
    pub fn deliver_connected(&self, session_id: WebRtcSessionId) {
        let sender = self.inner.lock().senders.get(&session_id).cloned();
        if let Some(mut tx) = sender {
            let _ = tx.try_send(BridgeLifecycleEvent::Connected);
        }
    }

    /// Forward a `Closed` lifecycle event. Removes the entry once
    /// delivered: the bridge will exit once it sees the close event,
    /// so keeping the sender around would leak a slot per closed
    /// session.
    pub fn deliver_closed(&self, session_id: WebRtcSessionId, reason: impl Into<String>) {
        let mut guard = self.inner.lock();
        if let Some(mut tx) = guard.senders.remove(&session_id) {
            let _ = tx.try_send(BridgeLifecycleEvent::Closed {
                reason: reason.into(),
            });
        }
    }

    /// Drop a session entry without emitting an event. Called by the
    /// bridge when it tears down on its own (e.g. cancel or remote
    /// bye) so the dispatcher doesn't keep stale senders alive.
    pub fn forget(&self, session_id: WebRtcSessionId) {
        self.inner.lock().senders.remove(&session_id);
    }

    /// Number of active subscriptions. Cheap; for diagnostics only.
    pub fn subscription_count(&self) -> usize {
        self.inner.lock().senders.len()
    }
}

#[async_trait]
impl BridgeLifecycleSource for LifecycleDispatcher {
    async fn subscribe(&self, session_id: WebRtcSessionId) -> mpsc::Receiver<BridgeLifecycleEvent> {
        let (tx, rx) = mpsc::channel(SUBSCRIBE_CAPACITY);
        self.inner.lock().senders.insert(session_id, tx);
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test(flavor = "current_thread")]
    async fn deliver_connected_routes_to_subscriber() {
        let dispatcher = LifecycleDispatcher::new();
        let id = WebRtcSessionId::new(1);
        let mut rx = dispatcher.subscribe(id).await;
        dispatcher.deliver_connected(id);
        match rx.next().await {
            Some(BridgeLifecycleEvent::Connected) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deliver_closed_routes_and_removes_subscriber() {
        let dispatcher = LifecycleDispatcher::new();
        let id = WebRtcSessionId::new(2);
        let mut rx = dispatcher.subscribe(id).await;
        assert_eq!(dispatcher.subscription_count(), 1);
        dispatcher.deliver_closed(id, "peer reset");
        match rx.next().await {
            Some(BridgeLifecycleEvent::Closed { reason }) => assert_eq!(reason, "peer reset"),
            other => panic!("unexpected: {other:?}"),
        }
        assert_eq!(dispatcher.subscription_count(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deliver_without_subscriber_is_noop() {
        let dispatcher = LifecycleDispatcher::new();
        // Should not panic; just dropped silently.
        dispatcher.deliver_connected(WebRtcSessionId::new(99));
        dispatcher.deliver_closed(WebRtcSessionId::new(99), "x");
        assert_eq!(dispatcher.subscription_count(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forget_drops_subscriber_without_event() {
        let dispatcher = LifecycleDispatcher::new();
        let id = WebRtcSessionId::new(3);
        let mut rx = dispatcher.subscribe(id).await;
        dispatcher.forget(id);
        // The sender side dropped; recv yields `None` not an event.
        let event = tokio::time::timeout(std::time::Duration::from_millis(50), rx.next()).await;
        assert!(matches!(event, Ok(None)), "expected drop, got {event:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscribe_replaces_existing_entry() {
        let dispatcher = LifecycleDispatcher::new();
        let id = WebRtcSessionId::new(4);
        let _rx1 = dispatcher.subscribe(id).await;
        let mut rx2 = dispatcher.subscribe(id).await;
        dispatcher.deliver_connected(id);
        // Only the second subscriber sees the event.
        match rx2.next().await {
            Some(BridgeLifecycleEvent::Connected) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
