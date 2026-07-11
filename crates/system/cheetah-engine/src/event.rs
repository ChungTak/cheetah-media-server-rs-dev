use async_trait::async_trait;
use cheetah_sdk::{EventBus, EventSubscriber, SystemEvent};
use tokio::sync::broadcast;

/// In-memory broadcast event bus implementation.
///
/// 内存广播事件总线实现。
pub struct LocalEventBus {
    tx: broadcast::Sender<SystemEvent>,
}

impl LocalEventBus {
    /// Create a broadcast channel with the given capacity.
    ///
    /// 用指定容量创建广播通道。
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self { tx }
    }
}

impl Default for LocalEventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

/// Subscriber that wraps a broadcast receiver and skips lagged events.
///
/// 包装广播接收器并跳过落后事件的订阅者。
struct QueueSubscriber {
    rx: broadcast::Receiver<SystemEvent>,
}

/// `EventSubscriber` implementation that loops over the broadcast receiver.
///
/// `EventSubscriber` 实现，循环读取广播接收器。
#[async_trait]
impl EventSubscriber for QueueSubscriber {
    async fn recv(&mut self) -> Option<SystemEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// `EventBus` implementation using a Tokio broadcast channel.
///
/// 使用 Tokio 广播通道的 `EventBus` 实现。
impl EventBus for LocalEventBus {
    fn publish(&self, event: SystemEvent) {
        let _ = self.tx.send(event);
    }

    fn subscribe(&self, _capacity: usize) -> Box<dyn EventSubscriber> {
        Box::new(QueueSubscriber {
            rx: self.tx.subscribe(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_event(phase: &str) -> SystemEvent {
        SystemEvent::System(cheetah_sdk::SystemLifecycleEvent {
            component: "engine".to_string(),
            phase: phase.to_string(),
            message: None,
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_published_event() {
        let bus = LocalEventBus::new(8);
        let mut sub = bus.subscribe(8);
        bus.publish(system_event("started"));
        let got = sub.recv().await.expect("event");
        match got {
            SystemEvent::System(event) => assert_eq!(event.phase, "started"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lagged_subscriber_continues_receiving_latest_events() {
        let bus = LocalEventBus::new(2);
        let mut sub = bus.subscribe(2);
        for idx in 0..6 {
            bus.publish(system_event(&format!("p{idx}")));
        }

        let first = sub.recv().await.expect("event");
        let second = sub.recv().await.expect("event");
        let phases = vec![
            match first {
                SystemEvent::System(event) => event.phase,
                other => panic!("unexpected event: {other:?}"),
            },
            match second {
                SystemEvent::System(event) => event.phase,
                other => panic!("unexpected event: {other:?}"),
            },
        ];
        assert!(
            phases.iter().all(|phase| phase == "p4" || phase == "p5"),
            "subscriber should continue with latest buffered events: {phases:?}"
        );
    }
}
