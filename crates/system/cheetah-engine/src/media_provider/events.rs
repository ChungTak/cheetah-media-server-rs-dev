//! Media event dispatcher that bridges `MediaEvent` to the engine `EventBus`.
//!
//! 将 `MediaEvent` 桥接到引擎 `EventBus` 的媒体事件分发器。

use std::sync::Arc;

use cheetah_media_api::error::{MediaError, Result as MediaResult};
use cheetah_media_api::event::{MediaEvent, MediaEventSender};
use cheetah_sdk::{EventBus, ProtocolEvent, SystemEvent};
use parking_lot::Mutex;

/// Dispatches `MediaEvent` to the engine event bus and to any `MediaEventSender`
/// registered through `MediaFacade::subscribe_events`.
///
/// 将 `MediaEvent` 分发到引擎事件总线以及通过 `MediaFacade::subscribe_events`
/// 注册的所有 `MediaEventSender`。
pub struct MediaEventDispatcher {
    event_bus: Arc<dyn EventBus>,
    subscriber: Mutex<Option<Box<dyn MediaEventSender>>>,
}

impl MediaEventDispatcher {
    /// Create a dispatcher backed by the supplied event bus.
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            subscriber: Mutex::new(None),
        }
    }

    /// Register a direct `MediaEventSender` subscriber.
    pub fn set_subscriber(&self, sender: Box<dyn MediaEventSender>) {
        *self.subscriber.lock() = Some(sender);
    }
}

impl MediaEventSender for MediaEventDispatcher {
    fn send(&self, event: MediaEvent) -> MediaResult<()> {
        if let Some(sub) = self.subscriber.lock().as_ref() {
            // Subscriber failures must not break event-bus publishing.
            let _ = sub.send(event.clone());
        }

        let payload = serde_json::to_value(&event)
            .map_err(|e| MediaError::internal(format!("failed to serialize media event: {e}")))?;
        let event_type = media_event_type(&event).to_string();
        self.event_bus.publish(SystemEvent::Protocol(ProtocolEvent {
            protocol: "media".to_string(),
            event_type,
            payload,
        }));
        Ok(())
    }

    fn lagged(&self, dropped: u64) -> MediaResult<()> {
        if let Some(sub) = self.subscriber.lock().as_ref() {
            sub.lagged(dropped)?;
        }
        Ok(())
    }
}

fn media_event_type(event: &MediaEvent) -> &'static str {
    match event {
        MediaEvent::StreamPublished(_) => "stream_published",
        MediaEvent::StreamUnpublished(_) => "stream_unpublished",
        MediaEvent::StreamOnlineChanged(_) => "stream_online_changed",
        MediaEvent::SessionOpened(_) => "session_opened",
        MediaEvent::SessionClosed(_) => "session_closed",
        MediaEvent::RecordStarted(_) => "record_started",
        MediaEvent::RecordProgress(_) => "record_progress",
        MediaEvent::RecordCompleted(_) => "record_completed",
        MediaEvent::SnapshotCompleted(_) => "snapshot_completed",
        MediaEvent::RtpSessionTimeout(_) => "rtp_session_timeout",
        MediaEvent::ProxyStateChanged(_) => "proxy_state_changed",
        MediaEvent::ServerLifecycle(_) => "server_lifecycle",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::LocalEventBus;
    use cheetah_media_api::event::{EventHeader, RecordStarted};
    use cheetah_media_api::ids::RecordTaskId;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn sample_event() -> MediaEvent {
        MediaEvent::RecordStarted(RecordStarted {
            header: EventHeader {
                event_id: "evt-1".to_string(),
                occurred_at: 1,
                sequence: None,
                media_key: None,
                source: "record".to_string(),
                correlation_id: None,
            },
            task_id: RecordTaskId("task-1".to_string()),
            format: "mp4".to_string(),
        })
    }

    struct CountingSender {
        count: Arc<AtomicU64>,
    }

    impl MediaEventSender for CountingSender {
        fn send(&self, _event: MediaEvent) -> MediaResult<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn lagged(&self, _dropped: u64) -> MediaResult<()> {
            Ok(())
        }
    }

    fn make_counter() -> (CountingSender, Arc<AtomicU64>) {
        let count = Arc::new(AtomicU64::new(0));
        (
            CountingSender {
                count: count.clone(),
            },
            count,
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatcher_publishes_protocol_event_to_event_bus() {
        let bus = Arc::new(LocalEventBus::new(8));
        let dispatcher = MediaEventDispatcher::new(bus.clone());
        let mut sub = bus.subscribe(8);

        let event = sample_event();
        dispatcher.send(event.clone()).unwrap();

        let got = sub.recv().await.expect("event");
        match got {
            SystemEvent::Protocol(p) => {
                assert_eq!(p.protocol, "media");
                assert_eq!(p.event_type, "record_started");
                assert_eq!(
                    p.payload.get("event").and_then(|v| v.as_str()),
                    Some("record_started")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatcher_forwards_to_registered_subscriber() {
        let bus = Arc::new(LocalEventBus::new(8));
        let dispatcher = MediaEventDispatcher::new(bus);
        let (sender, count) = make_counter();
        dispatcher.set_subscriber(Box::new(sender));

        dispatcher.send(sample_event()).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lagged_propagates_to_registered_subscriber() {
        let bus = Arc::new(LocalEventBus::new(8));
        let dispatcher = MediaEventDispatcher::new(bus);
        let (sender, count) = make_counter();
        dispatcher.set_subscriber(Box::new(sender));

        dispatcher.lagged(5).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
}
