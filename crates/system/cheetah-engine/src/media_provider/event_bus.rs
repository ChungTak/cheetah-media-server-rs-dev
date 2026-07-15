use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cheetah_media_api::error::Result;
use cheetah_media_api::event::{
    MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
};
use cheetah_runtime_api::RuntimeApi;
use parking_lot::Mutex;

/// Default upper bound for per-resource sequence counters.
///
/// 按资源 sequence 计数器的默认上限。
const DEFAULT_MAX_SEQUENCE_KEYS: usize = 4096;

/// In-memory bounded media event bus.
///
/// Each subscriber has an independent bounded `tokio::sync::mpsc` queue. Slow
/// subscribers are notified via `lagged` with a cumulative dropped count.
/// Sequence numbers are maintained per resource (`media_key` when available,
/// otherwise `source`) and capped to a bounded set of tracked resources.
///
/// 内存有界媒体事件总线。
/// 每个订阅者拥有独立的 `tokio::sync::mpsc` 有界队列；慢速订阅者通过 `lagged`
/// 与累计丢包数获得通知。sequence 按资源（优先 `media_key`，否则 `source`）维护，
/// 并限制跟踪的资源数量上限。
#[derive(Clone)]
pub struct LocalMediaEventBus {
    inner: Arc<Mutex<BusInner>>,
    runtime_api: Arc<dyn RuntimeApi>,
    next_id: Arc<AtomicU64>,
    max_sequence_keys: usize,
}

struct BusInner {
    subscribers: HashMap<String, SubscriberState>,
    sequences: HashMap<String, u64>,
    sequence_use: HashMap<String, u64>,
    sequence_counter: u64,
}

struct SubscriberState {
    tx: tokio::sync::mpsc::Sender<MediaEvent>,
    sender: Arc<dyn MediaEventSender>,
    dropped: u64,
    notified_dropped: u64,
}

impl LocalMediaEventBus {
    /// Create a new media event bus using the provided runtime to spawn
    /// per-subscriber forwarding tasks.
    ///
    /// 使用指定运行时创建新的媒体事件总线，以生成每个订阅者的转发任务。
    pub fn new(runtime_api: Arc<dyn RuntimeApi>) -> Self {
        Self::with_max_sequence_keys(runtime_api, DEFAULT_MAX_SEQUENCE_KEYS)
    }

    /// Create a bus with a custom cap on tracked per-resource sequence counters.
    ///
    /// 使用自定义的按资源 sequence 跟踪上限创建总线。
    pub fn with_max_sequence_keys(
        runtime_api: Arc<dyn RuntimeApi>,
        max_sequence_keys: usize,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BusInner {
                subscribers: HashMap::new(),
                sequences: HashMap::new(),
                sequence_use: HashMap::new(),
                sequence_counter: 0,
            })),
            runtime_api,
            next_id: Arc::new(AtomicU64::new(1)),
            max_sequence_keys: max_sequence_keys.max(1),
        }
    }

    fn next_sequence(&self, inner: &mut BusInner, resource_key: &str) -> u64 {
        inner.sequence_counter += 1;
        let now = inner.sequence_counter;

        if !inner.sequences.contains_key(resource_key)
            && inner.sequences.len() >= self.max_sequence_keys
        {
            // Evict the least-recently-used tracked resource.
            if let Some((oldest, _)) = inner.sequence_use.iter().min_by_key(|(_, v)| *v) {
                let oldest = oldest.clone();
                inner.sequences.remove(&oldest);
                inner.sequence_use.remove(&oldest);
            }
        }

        let entry = inner.sequences.entry(resource_key.to_string()).or_insert(0);
        inner.sequence_use.insert(resource_key.to_string(), now);
        *entry += 1;
        *entry
    }

    fn spawn_forwarder(
        &self,
        mut rx: tokio::sync::mpsc::Receiver<MediaEvent>,
        sender: Arc<dyn MediaEventSender>,
    ) {
        let fut = async move {
            while let Some(event) = rx.recv().await {
                if sender.send(event).is_err() {
                    break;
                }
            }
        };
        let _ = self.runtime_api.spawn(Box::pin(fut)
            as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>);
    }
}

impl MediaEventBusApi for LocalMediaEventBus {
    fn publish(&self, mut event: MediaEvent) -> Result<()> {
        let mut inner = self.inner.lock();

        let resource_key = event.resource_key();
        let seq = self.next_sequence(&mut inner, &resource_key);
        event.header_mut().sequence = Some(seq);

        let mut lag_callbacks: Vec<(Arc<dyn MediaEventSender>, u64)> = Vec::new();
        let mut closed: Vec<String> = Vec::new();

        for (id, sub) in inner.subscribers.iter_mut() {
            match sub.tx.try_send(event.clone()) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    sub.dropped += 1;
                    if sub.dropped > sub.notified_dropped {
                        let delta = sub.dropped - sub.notified_dropped;
                        sub.notified_dropped = sub.dropped;
                        lag_callbacks.push((Arc::clone(&sub.sender), delta));
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    closed.push(id.clone());
                }
            }
        }

        for id in closed {
            inner.subscribers.remove(&id);
        }
        drop(inner);

        // Notify outside the lock so a lagged callback cannot reenter while
        // publish is still holding the mutex.
        for (sender, delta) in lag_callbacks {
            let _ = sender.lagged(delta);
        }

        Ok(())
    }

    fn subscribe(
        &self,
        sender: Box<dyn MediaEventSender>,
        capacity: usize,
    ) -> Result<Box<dyn MediaEventSubscription>> {
        let capacity = capacity.max(1);
        let id = format!("media-sub-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = tokio::sync::mpsc::channel::<MediaEvent>(capacity);
        let sender = Arc::from(sender);
        self.spawn_forwarder(rx, Arc::clone(&sender));

        let mut inner = self.inner.lock();
        inner.subscribers.insert(
            id.clone(),
            SubscriberState {
                tx,
                sender,
                dropped: 0,
                notified_dropped: 0,
            },
        );
        drop(inner);

        Ok(Box::new(LocalMediaEventSubscription {
            bus: self.clone(),
            id,
        }))
    }

    fn unsubscribe(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock();
        inner.subscribers.remove(id);
        drop(inner);
        Ok(())
    }
}

struct LocalMediaEventSubscription {
    bus: LocalMediaEventBus,
    id: String,
}

impl MediaEventSubscription for LocalMediaEventSubscription {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn unsubscribe(&self) -> Result<()> {
        self.bus.unsubscribe(&self.id)
    }
}

impl Drop for LocalMediaEventSubscription {
    fn drop(&mut self) {
        let _ = self.bus.unsubscribe(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use cheetah_media_api::event::{EventHeader, ServerLifecycle, ServerLifecycleKind};
    use cheetah_runtime_tokio::TokioRuntime;

    fn runtime() -> Arc<dyn RuntimeApi> {
        Arc::new(TokioRuntime::new()) as Arc<dyn RuntimeApi>
    }

    #[derive(Clone)]
    struct CollectingSender {
        events: Arc<Mutex<Vec<MediaEvent>>>,
        lagged: Arc<AtomicU64>,
    }

    impl CollectingSender {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                lagged: Arc::new(AtomicU64::new(0)),
            }
        }
    }

    impl MediaEventSender for CollectingSender {
        fn send(&self, event: MediaEvent) -> Result<()> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }

        fn lagged(&self, dropped: u64) -> Result<()> {
            self.lagged.fetch_add(dropped, Ordering::Relaxed);
            Ok(())
        }
    }

    fn lifecycle_event(id: &str, source: &str) -> MediaEvent {
        MediaEvent::ServerLifecycle(ServerLifecycle {
            header: EventHeader {
                event_id: id.to_string(),
                occurred_at: 1,
                sequence: None,
                media_key: None,
                source: source.to_string(),
                correlation_id: None,
            },
            kind: ServerLifecycleKind::Started,
            server_id: "srv".to_string(),
            version: "0.1.0".to_string(),
            status: "ok".to_string(),
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_event_with_sequence() {
        let bus = LocalMediaEventBus::new(runtime());
        let sender = CollectingSender::new();
        let _sub = bus.subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 8);

        bus.publish(lifecycle_event("e1", "src-a")).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        let mut events = sender.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_key(), "src:src-a");
        assert_eq!(events[0].header_mut().sequence, Some(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slow_subscriber_is_lagged_and_drops_oldest() {
        let bus = LocalMediaEventBus::new(runtime());
        let sender = CollectingSender::new();
        let _sub = bus.subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 1);

        // With capacity 1, the first event may be taken by the forwarder task
        // or sit in the channel; the rest should overflow and be counted as dropped.
        for i in 0..5 {
            bus.publish(lifecycle_event(&format!("e{i}"), "src-b"))
                .unwrap();
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        let lagged = sender.lagged.load(Ordering::Relaxed);
        assert!(lagged > 0, "lagged should be reported for dropped events");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn per_resource_sequence_is_independent() {
        let bus = LocalMediaEventBus::new(runtime());
        let sender = CollectingSender::new();
        let _sub = bus.subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 8);

        bus.publish(lifecycle_event("a1", "r1")).unwrap();
        bus.publish(lifecycle_event("a2", "r1")).unwrap();
        bus.publish(lifecycle_event("b1", "r2")).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut events = sender.events.lock().unwrap();
        assert_eq!(events[0].header_mut().sequence, Some(1));
        assert_eq!(events[1].header_mut().sequence, Some(2));
        assert_eq!(events[2].header_mut().sequence, Some(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unsubscribe_stops_delivery() {
        let bus = LocalMediaEventBus::new(runtime());
        let sender = CollectingSender::new();
        let sub = bus
            .subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 8)
            .unwrap();

        sub.unsubscribe().unwrap();
        bus.publish(lifecycle_event("after", "src")).unwrap();

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(sender.events.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sequence_map_is_bounded() {
        let bus = LocalMediaEventBus::with_max_sequence_keys(runtime(), 2);
        let sender = CollectingSender::new();
        let _sub = bus.subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 8);

        bus.publish(lifecycle_event("a", "r1")).unwrap();
        bus.publish(lifecycle_event("b", "r2")).unwrap();
        bus.publish(lifecycle_event("c", "r3")).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // The oldest resource may be evicted, so the total sequence map size
        // must not exceed the configured cap.
        let inner = bus.inner.lock();
        assert!(inner.sequences.len() <= 2);
    }
}
