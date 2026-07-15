use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cheetah_media_api::error::Result;
use cheetah_media_api::event::{
    MediaEvent, MediaEventBusApi, MediaEventSender, MediaEventSubscription,
};
use parking_lot::Mutex;

/// In-memory bounded media event bus.
///
/// Each subscriber has an independent bounded queue. Slow subscribers are
/// notified via `lagged` and a cumulative `dropped` count. Sequence numbers
/// are maintained per resource (`media_key` when available, otherwise `source`).
///
/// 内存有界媒体事件总线。
/// 每个订阅者拥有独立的有界队列；慢速订阅者通过 `lagged` 与累计丢包数获得通知。
/// sequence 按资源（优先 `media_key`，否则 `source`）维护。
#[derive(Clone)]
pub struct LocalMediaEventBus {
    inner: Arc<Mutex<BusState>>,
    next_id: Arc<AtomicU64>,
}

struct BusState {
    subscribers: HashMap<String, SubscriberState>,
    sequences: HashMap<String, u64>,
}

struct SubscriberState {
    sender: Arc<dyn MediaEventSender>,
    queue: VecDeque<MediaEvent>,
    capacity: usize,
    dropped: u64,
    notified_dropped: u64,
}

impl LocalMediaEventBus {
    /// Create a new media event bus.
    ///
    /// 创建新的媒体事件总线。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BusState {
                subscribers: HashMap::new(),
                sequences: HashMap::new(),
            })),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl Default for LocalMediaEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaEventBusApi for LocalMediaEventBus {
    fn publish(&self, mut event: MediaEvent) -> Result<()> {
        let mut state = self.inner.lock();

        let resource_key = event.resource_key();
        let seq = state.sequences.entry(resource_key).or_insert(0);
        *seq += 1;
        event.header_mut().sequence = Some(*seq);

        let mut lag_callbacks: Vec<(Arc<dyn MediaEventSender>, u64)> = Vec::new();

        for sub in state.subscribers.values_mut() {
            sub.queue.push_back(event.clone());
            if sub.queue.len() > sub.capacity {
                sub.queue.pop_front();
                sub.dropped += 1;
            }

            // Deliver as many queued events as the sender can accept right now.
            while let Some(ev) = sub.queue.front() {
                if sub.sender.send(ev.clone()).is_ok() {
                    sub.queue.pop_front();
                } else {
                    break;
                }
            }

            if sub.dropped > sub.notified_dropped {
                let delta = sub.dropped - sub.notified_dropped;
                sub.notified_dropped = sub.dropped;
                lag_callbacks.push((Arc::clone(&sub.sender), delta));
            }
        }

        drop(state);

        // Notify outside the lock so a lagged callback cannot reenter the bus.
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
        let mut state = self.inner.lock();
        state.subscribers.insert(
            id.clone(),
            SubscriberState {
                sender: Arc::from(sender),
                queue: VecDeque::with_capacity(capacity.min(64)),
                capacity,
                dropped: 0,
                notified_dropped: 0,
            },
        );
        drop(state);
        Ok(Box::new(LocalMediaEventSubscription {
            bus: self.clone(),
            id,
        }))
    }

    fn unsubscribe(&self, id: &str) -> Result<()> {
        let mut state = self.inner.lock();
        state.subscribers.remove(id);
        drop(state);
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

    use super::*;
    use cheetah_media_api::event::{EventHeader, ServerLifecycle, ServerLifecycleKind};
    use cheetah_media_api::MediaError;

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

    #[test]
    fn subscriber_receives_event_with_sequence() {
        let bus = LocalMediaEventBus::new();
        let sender = CollectingSender::new();
        let _sub = bus.subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 8);

        bus.publish(lifecycle_event("e1", "src-a")).unwrap();

        let mut events = sender.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].resource_key(), "src:src-a");
        assert_eq!(events[0].header_mut().sequence, Some(1));
    }

    #[test]
    fn slow_subscriber_is_lagged_and_drops_oldest() {
        let bus = LocalMediaEventBus::new();

        // Sender rejects after one accepted event, exercising backpressure handling.
        #[derive(Clone)]
        struct LaggySender {
            count: Arc<AtomicU64>,
            events: Arc<Mutex<Vec<MediaEvent>>>,
            lagged: Arc<AtomicU64>,
        }
        impl MediaEventSender for LaggySender {
            fn send(&self, event: MediaEvent) -> Result<()> {
                if self.count.fetch_add(1, Ordering::Relaxed) >= 1 {
                    return Err(MediaError::unavailable("backpressure"));
                }
                self.events.lock().unwrap().push(event);
                Ok(())
            }
            fn lagged(&self, dropped: u64) -> Result<()> {
                self.lagged.fetch_add(dropped, Ordering::Relaxed);
                Ok(())
            }
        }
        let laggy = LaggySender {
            count: Arc::new(AtomicU64::new(0)),
            events: Arc::new(Mutex::new(Vec::new())),
            lagged: Arc::new(AtomicU64::new(0)),
        };
        let _sub = bus.subscribe(Box::new(laggy.clone()) as Box<dyn MediaEventSender>, 2);

        for i in 0..5 {
            bus.publish(lifecycle_event(&format!("e{i}"), "src-b"))
                .unwrap();
        }

        let events = laggy.events.lock().unwrap();
        // One event is accepted, the rest is dropped once the queue is full.
        assert!(!events.is_empty());
        assert!(laggy.lagged.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn per_resource_sequence_is_independent() {
        let bus = LocalMediaEventBus::new();
        let sender = CollectingSender::new();
        let _sub = bus.subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 8);

        bus.publish(lifecycle_event("a1", "r1")).unwrap();
        bus.publish(lifecycle_event("a2", "r1")).unwrap();
        bus.publish(lifecycle_event("b1", "r2")).unwrap();

        let mut events = sender.events.lock().unwrap();
        assert_eq!(events[0].header_mut().sequence, Some(1));
        assert_eq!(events[1].header_mut().sequence, Some(2));
        assert_eq!(events[2].header_mut().sequence, Some(1));
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let bus = LocalMediaEventBus::new();
        let sender = CollectingSender::new();
        let sub = bus
            .subscribe(Box::new(sender.clone()) as Box<dyn MediaEventSender>, 8)
            .unwrap();

        sub.unsubscribe().unwrap();
        bus.publish(lifecycle_event("after", "src")).unwrap();

        assert!(sender.events.lock().unwrap().is_empty());
    }
}
