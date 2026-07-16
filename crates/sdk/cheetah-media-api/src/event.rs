use serde::{Deserialize, Serialize};

use crate::ids::*;
use crate::model::*;

/// Internal media-domain event.
///
/// 内部媒体领域事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum MediaEvent {
    StreamPublished(StreamPublished),
    StreamUnpublished(StreamUnpublished),
    StreamOnlineChanged(StreamOnlineChanged),
    SessionOpened(SessionOpened),
    SessionClosed(SessionClosed),
    RecordStarted(RecordStarted),
    RecordProgress(RecordProgress),
    RecordCompleted(RecordCompleted),
    SnapshotCompleted(SnapshotCompleted),
    RtpSessionTimeout(RtpSessionTimeout),
    ProxyStateChanged(ProxyStateChanged),
    ServerLifecycle(ServerLifecycle),
}

/// Common metadata for every event.
///
/// 每个事件共有的元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventHeader {
    pub event_id: String,
    pub occurred_at: i64,
    pub sequence: Option<u64>,
    pub media_key: Option<MediaKey>,
    pub source: String,
    pub correlation_id: Option<String>,
}

/// A stream was published.
///
/// 流已发布。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamPublished {
    pub header: EventHeader,
    pub protocol: String,
    pub remote_endpoint: Option<String>,
    pub session_id: SessionId,
}

/// A stream was unpublished.
///
/// 流已结束发布。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamUnpublished {
    pub header: EventHeader,
    pub session_id: SessionId,
    pub reason: CloseReason,
}

/// Online state of a stream changed.
///
/// 流在线状态发生变化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamOnlineChanged {
    pub header: EventHeader,
    pub online: OnlineState,
    pub schema: Option<MediaSchema>,
}

/// A session was opened.
///
/// 会话已打开。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOpened {
    pub header: EventHeader,
    pub kind: SessionKind,
    pub session_id: SessionId,
    pub remote_endpoint: Option<String>,
    pub protocol: String,
}

/// A session was closed.
///
/// 会话已关闭。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionClosed {
    pub header: EventHeader,
    pub kind: SessionKind,
    pub session_id: SessionId,
    pub reason: CloseReason,
}

/// A record task started.
///
/// 录制任务已开始。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordStarted {
    pub header: EventHeader,
    pub task_id: RecordTaskId,
    pub format: String,
}

/// Record progress event.
///
/// 录制进度事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordProgress {
    pub header: EventHeader,
    pub task_id: RecordTaskId,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub file_path: Option<String>,
}

/// Record task completed.
///
/// 录制任务完成。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordCompleted {
    pub header: EventHeader,
    pub task_id: RecordTaskId,
    pub format: String,
    pub file_path: String,
    pub file_size: u64,
    pub time_len_ms: u64,
    pub folder: String,
    pub url: Option<String>,
}

/// Snapshot completed.
///
/// 快照完成。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotCompleted {
    pub header: EventHeader,
    pub snapshot_id: SnapshotId,
    pub path_handle: FileHandle,
    pub url: Option<String>,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub size_bytes: u64,
}

/// RTP session timed out.
///
/// RTP 会话超时。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtpSessionTimeout {
    pub header: EventHeader,
    pub session_id: RtpSessionId,
    pub local_port: Option<u16>,
    pub tcp_mode: Option<RtpTcpMode>,
    pub reuse_port: bool,
    pub ssrc: Option<u32>,
}

/// Proxy state changed.
///
/// 代理状态变化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyStateChanged {
    pub header: EventHeader,
    pub proxy_id: ProxyId,
    pub state: ProxyState,
    pub last_error: Option<String>,
}

/// Server lifecycle event.
///
/// 服务器生命周期事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerLifecycle {
    pub header: EventHeader,
    pub kind: ServerLifecycleKind,
    pub server_id: String,
    pub version: String,
    pub status: String,
}

/// Server lifecycle kind.
///
/// 服务器生命周期类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerLifecycleKind {
    Started,
    Exited,
    Keepalive,
}

/// Trait for sending media events.
///
/// 发送媒体事件的 trait。
pub trait MediaEventSender: Send + Sync {
    fn send(&self, event: MediaEvent) -> crate::error::Result<()>;
    fn lagged(&self, dropped: u64) -> crate::error::Result<()>;
}

/// Trait for subscribing to media events.
///
/// 订阅媒体事件的 trait。
pub trait MediaEventSubscriber: Send + Sync {
    fn on_event(&self, event: MediaEvent) -> crate::error::Result<()>;
    fn on_lagged(&self, dropped: u64) -> crate::error::Result<()>;
}

/// A subscription to the media event bus.
///
/// Dropping the handle automatically unsubscribes; `unsubscribe` allows
/// explicit cancellation before the handle goes out of scope.
///
/// 媒体事件总线的订阅句柄。
/// 句柄 Drop 时自动取消订阅；`unsubscribe` 允许在作用域结束前显式取消。
pub trait MediaEventSubscription: Send + Sync {
    fn id(&self) -> String;
    fn unsubscribe(&self) -> crate::error::Result<()>;
}

/// Bounded, typed media event bus.
///
/// - Each subscriber has its own bounded queue.
/// - Slow subscribers receive `lagged` callbacks and a cumulative dropped count.
/// - Sequence numbers are maintained per resource (`media_key` or `source`).
///
/// 有界、类型化的媒体事件总线。
/// - 每个订阅者拥有独立的有界队列。
/// - 慢速订阅者会收到 `lagged` 回调与累计丢包计数。
/// - sequence 按资源（`media_key` 或 `source`）维护。
pub trait MediaEventBusApi: Send + Sync {
    fn publish(&self, event: MediaEvent) -> crate::error::Result<()>;
    fn subscribe(
        &self,
        sender: Box<dyn MediaEventSender>,
        capacity: usize,
    ) -> crate::error::Result<Box<dyn MediaEventSubscription>>;
    fn unsubscribe(&self, id: &str) -> crate::error::Result<()>;
}

impl MediaEvent {
    /// Read-only access to the shared event header.
    ///
    /// 获取共享事件头部的只读引用。
    pub fn header(&self) -> &EventHeader {
        match self {
            MediaEvent::StreamPublished(e) => &e.header,
            MediaEvent::StreamUnpublished(e) => &e.header,
            MediaEvent::StreamOnlineChanged(e) => &e.header,
            MediaEvent::SessionOpened(e) => &e.header,
            MediaEvent::SessionClosed(e) => &e.header,
            MediaEvent::RecordStarted(e) => &e.header,
            MediaEvent::RecordProgress(e) => &e.header,
            MediaEvent::RecordCompleted(e) => &e.header,
            MediaEvent::SnapshotCompleted(e) => &e.header,
            MediaEvent::RtpSessionTimeout(e) => &e.header,
            MediaEvent::ProxyStateChanged(e) => &e.header,
            MediaEvent::ServerLifecycle(e) => &e.header,
        }
    }

    /// Mutable access to the shared event header.
    ///
    /// 获取共享事件头部的可变引用。
    pub fn header_mut(&mut self) -> &mut EventHeader {
        match self {
            MediaEvent::StreamPublished(e) => &mut e.header,
            MediaEvent::StreamUnpublished(e) => &mut e.header,
            MediaEvent::StreamOnlineChanged(e) => &mut e.header,
            MediaEvent::SessionOpened(e) => &mut e.header,
            MediaEvent::SessionClosed(e) => &mut e.header,
            MediaEvent::RecordStarted(e) => &mut e.header,
            MediaEvent::RecordProgress(e) => &mut e.header,
            MediaEvent::RecordCompleted(e) => &mut e.header,
            MediaEvent::SnapshotCompleted(e) => &mut e.header,
            MediaEvent::RtpSessionTimeout(e) => &mut e.header,
            MediaEvent::ProxyStateChanged(e) => &mut e.header,
            MediaEvent::ServerLifecycle(e) => &mut e.header,
        }
    }

    /// Return a key used for per-resource sequence numbering.
    ///
    /// 返回用于按资源维护 sequence 的键。
    pub fn resource_key(&self) -> String {
        let header = match self {
            MediaEvent::StreamPublished(e) => &e.header,
            MediaEvent::StreamUnpublished(e) => &e.header,
            MediaEvent::StreamOnlineChanged(e) => &e.header,
            MediaEvent::SessionOpened(e) => &e.header,
            MediaEvent::SessionClosed(e) => &e.header,
            MediaEvent::RecordStarted(e) => &e.header,
            MediaEvent::RecordProgress(e) => &e.header,
            MediaEvent::RecordCompleted(e) => &e.header,
            MediaEvent::SnapshotCompleted(e) => &e.header,
            MediaEvent::RtpSessionTimeout(e) => &e.header,
            MediaEvent::ProxyStateChanged(e) => &e.header,
            MediaEvent::ServerLifecycle(e) => &e.header,
        };
        header
            .media_key
            .as_ref()
            .map(|k| format!("mk:{}", k.to_canonical()))
            .unwrap_or_else(|| format!("src:{}", header.source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_round_trip_serialization() {
        let header = EventHeader {
            event_id: "evt-1".to_string(),
            occurred_at: 1,
            sequence: Some(1),
            media_key: None,
            source: "test".to_string(),
            correlation_id: None,
        };
        let event = MediaEvent::ServerLifecycle(ServerLifecycle {
            header,
            kind: ServerLifecycleKind::Started,
            server_id: "srv-1".to_string(),
            version: "0.1.0".to_string(),
            status: "ok".to_string(),
        });
        let json = serde_json::to_string(&event).unwrap();
        let de: MediaEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(de, event);
    }
}
