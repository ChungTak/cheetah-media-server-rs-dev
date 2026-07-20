//! Runtime-neutral controlled-media event types for the control-plane journal.
//!
//! 控制面 journal 的运行时无关受控媒体事件类型。

use serde::{Deserialize, Serialize};

use crate::cursor::OpaqueCursor;
use crate::error::MediaError;
use crate::fencing::{ControlledResourceRef, ResourceOrigin};
use crate::ids::{
    MediaBindingId, MediaKey, MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId,
    MediaSessionId, MessageId, OwnerEpoch, ResourceGeneration, TenantId,
};
use crate::resource_filter::{ResourceFilter, ResourceState};

/// Opaque, globally-unique identifier for an event in the durable journal.
///
/// 可重放事件日志中事件的全局唯一标识。
pub type EventId = MessageId;

/// Monotonic sequence number scoped to a single media-node instance epoch.
///
/// 单个媒体节点实例 epoch 内的单调序列号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSequence(pub u64);

impl EventSequence {
    /// Return the raw sequence value.
    pub fn value(&self) -> u64 {
        self.0
    }
}

/// Header shared by every controlled-media event.
///
/// 每个受控媒体事件共享的 header。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlledEventHeader {
    pub event_id: EventId,
    pub tenant_id: TenantId,
    pub media_node_id: MediaNodeId,
    pub media_node_instance_id: MediaNodeInstanceId,
    pub media_node_instance_epoch: MediaNodeInstanceEpoch,
    pub sequence: EventSequence,
    pub occurred_at: i64,
    pub correlation_id: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// Payload variants for a controlled-media event.
///
/// Payloads are intentionally free of secrets, user info, internal paths, or
/// unsanitized last errors. Any failure details are recorded as a safe
/// `MediaError` inside `ResourceStateChanged`.
///
/// 受控媒体事件的 payload 变体。payload 不携带 secret、userinfo、内部路径或
/// 未脱敏的 last error。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlledEventPayload {
    ResourceStateChanged(ResourceStateChanged),
    StreamOnline(StreamOnline),
    StreamOffline(StreamOffline),
    RtpSessionTimeout(RtpSessionTimeout),
    ProxyStateChanged(ProxyStateChanged),
    RecordCompleted(RecordCompleted),
    SnapshotCompleted(SnapshotCompleted),
    PlaybackCompleted(PlaybackCompleted),
    ProcessingCompleted(ProcessingCompleted),
    NodeLifecycle(NodeLifecycle),
    Gap(EventGap),
}

/// A controlled-media event with a fixed header and typed payload.
///
/// 带固定 header 和类型化 payload 的受控媒体事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlledMediaEvent {
    pub header: ControlledEventHeader,
    pub payload: ControlledEventPayload,
}

impl ControlledMediaEvent {
    /// Resource reference derived from the event payload.
    ///
    /// Returns `Some` only for `ResourceStateChanged`, which carries the
    /// resource kind, handle, owner epoch and generation needed for a full
    /// `ControlledResourceRef`. Other payload variants do not represent a
    /// single controlled resource and return `None`.
    pub fn resource_ref(&self) -> Option<ControlledResourceRef> {
        match &self.payload {
            ControlledEventPayload::ResourceStateChanged(p) => Some(ControlledResourceRef {
                tenant_id: self.header.tenant_id.clone(),
                media_session_id: p.media_session_id.clone(),
                media_binding_id: p.media_binding_id.clone(),
                resource_kind: p.resource_kind.clone(),
                resource_handle: p.resource_handle.clone(),
                owner_epoch: p.owner_epoch,
                node_instance_epoch: self.header.media_node_instance_epoch,
                generation: p.generation,
                origin: ResourceOrigin::default(),
            }),
            ControlledEventPayload::StreamOnline(_)
            | ControlledEventPayload::StreamOffline(_)
            | ControlledEventPayload::RtpSessionTimeout(_)
            | ControlledEventPayload::ProxyStateChanged(_)
            | ControlledEventPayload::RecordCompleted(_)
            | ControlledEventPayload::SnapshotCompleted(_)
            | ControlledEventPayload::PlaybackCompleted(_)
            | ControlledEventPayload::ProcessingCompleted(_)
            | ControlledEventPayload::NodeLifecycle(_)
            | ControlledEventPayload::Gap(_) => None,
        }
    }
}

/// Resource state transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceStateChanged {
    pub resource_kind: String,
    pub resource_handle: String,
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    pub previous_state: ResourceState,
    pub new_state: ResourceState,
    pub owner_epoch: OwnerEpoch,
    pub generation: ResourceGeneration,
    pub media_key: Option<MediaKey>,
    /// Safe last error, if any. Secrets and internal paths must be removed.
    pub last_error: Option<MediaError>,
}

/// A stream came online.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOnline {
    pub session_id: MediaSessionId,
    pub media_key: MediaKey,
}

/// A stream went offline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOffline {
    pub session_id: MediaSessionId,
    pub media_key: MediaKey,
}

/// An RTP session timed out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtpSessionTimeout {
    pub session_id: MediaSessionId,
    pub media_key: MediaKey,
}

/// A proxy source changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyStateChanged {
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    /// Sanitized source identifier, free of credentials or internal paths.
    pub source: String,
    pub active: bool,
}

/// A record task completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCompleted {
    pub media_session_id: Option<MediaSessionId>,
    pub file_handle: String,
    pub duration_ms: u64,
}

/// A snapshot was completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCompleted {
    pub media_session_id: Option<MediaSessionId>,
    pub file_handle: String,
    pub width: u32,
    pub height: u32,
}

/// A playback task completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackCompleted {
    pub media_session_id: Option<MediaSessionId>,
    pub file_handle: String,
    pub duration_ms: u64,
}

/// A processing job completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingCompleted {
    pub media_session_id: Option<MediaSessionId>,
    pub output_media_key: Option<MediaKey>,
    pub output_file_handle: Option<String>,
}

/// Node lifecycle transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLifecycle {
    pub previous_state: String,
    pub new_state: String,
    pub instance_epoch: MediaNodeInstanceEpoch,
}

/// Gap event delivered when a subscriber's resume sequence is older than the
/// journal retention floor.
///
/// 当订阅者的恢复 sequence 早于 journal 保留底限时发送的 gap 事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventGap {
    pub requested_sequence: EventSequence,
    pub first_available_sequence: EventSequence,
    pub instance_epoch: MediaNodeInstanceEpoch,
    pub reconciliation_required: bool,
}

/// Request to subscribe to the durable event stream.
///
/// 订阅可重放事件流的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSubscribeRequest {
    pub tenant_id: TenantId,
    pub filter: ResourceFilter,
    pub resume_cursor: Option<OpaqueCursor>,
    pub max_batch: u32,
    pub max_bytes: u64,
}

/// Response page from a durable event stream subscription.
///
/// 可重放事件流订阅返回的页。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventSubscribeResponse {
    pub events: Vec<ControlledMediaEvent>,
    pub next_cursor: Option<OpaqueCursor>,
}

/// Limits enforced per event subscriber to prevent slow consumers.
///
/// 防止慢消费者的每个事件订阅者限制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriberLimits {
    pub queue_capacity: u32,
    pub max_batch: u32,
    pub max_bytes: u64,
    pub idle_deadline_ms: u64,
    pub max_subscribers: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AppName, StreamName, VhostName};

    fn header() -> ControlledEventHeader {
        ControlledEventHeader {
            event_id: MessageId::new("msg-1").unwrap(),
            tenant_id: TenantId::new("tenant-1").unwrap(),
            media_node_id: MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            media_node_instance_id: MediaNodeInstanceId::new(
                "550e8401-e29b-41d4-a716-446655440001",
            )
            .unwrap(),
            media_node_instance_epoch: MediaNodeInstanceEpoch(42),
            sequence: EventSequence(7),
            occurred_at: 1_000_000,
            correlation_id: None,
            traceparent: None,
            tracestate: None,
        }
    }

    #[test]
    fn event_round_trips_through_json() {
        let event = ControlledMediaEvent {
            header: header(),
            payload: ControlledEventPayload::StreamOnline(StreamOnline {
                session_id: MediaSessionId::new("550e8400-e29b-41d4-a716-446655440002").unwrap(),
                media_key: MediaKey {
                    vhost: VhostName::default(),
                    app: AppName::new("app").unwrap(),
                    stream: StreamName::new("stream").unwrap(),
                    schema: None,
                },
            }),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: ControlledMediaEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
    }

    #[test]
    fn resource_ref_uses_header_tenant_and_epoch() {
        let event = ControlledMediaEvent {
            header: header(),
            payload: ControlledEventPayload::ResourceStateChanged(ResourceStateChanged {
                resource_kind: "session".to_string(),
                resource_handle: "h1".to_string(),
                media_session_id: None,
                media_binding_id: None,
                previous_state: ResourceState::Pending,
                new_state: ResourceState::Active,
                owner_epoch: OwnerEpoch(7),
                generation: ResourceGeneration(3),
                media_key: None,
                last_error: None,
            }),
        };
        let r = event
            .resource_ref()
            .expect("resource ref from state changed");
        assert_eq!(r.tenant_id, event.header.tenant_id);
        assert_eq!(
            r.node_instance_epoch,
            event.header.media_node_instance_epoch
        );
        assert_eq!(r.resource_kind, "session");
        assert_eq!(r.resource_handle, "h1");
        assert_eq!(r.owner_epoch, OwnerEpoch(7));
        assert_eq!(r.generation, ResourceGeneration(3));
    }

    #[test]
    fn non_state_payloads_return_no_resource_ref() {
        let event = ControlledMediaEvent {
            header: header(),
            payload: ControlledEventPayload::StreamOnline(StreamOnline {
                session_id: MediaSessionId::new("550e8400-e29b-41d4-a716-446655440002").unwrap(),
                media_key: MediaKey {
                    vhost: VhostName::default(),
                    app: AppName::new("app").unwrap(),
                    stream: StreamName::new("stream").unwrap(),
                    schema: None,
                },
            }),
        };
        assert!(event.resource_ref().is_none());
    }

    #[test]
    fn subscribe_request_and_limits_round_trip() {
        let req = EventSubscribeRequest {
            tenant_id: TenantId::new("tenant-1").unwrap(),
            filter: ResourceFilter {
                tenant_id: TenantId::new("tenant-1").unwrap(),
                media_session_id: None,
                media_binding_id: None,
                resource_handle: None,
                media_key: None,
                idempotency_key: None,
                state: None,
                non_terminal: false,
                owner_epoch: None,
                node_instance_epoch: None,
                updated_before_ms: None,
                updated_after_ms: None,
            },
            resume_cursor: None,
            max_batch: 100,
            max_bytes: 1_000_000,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: EventSubscribeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);

        let limits = SubscriberLimits {
            queue_capacity: 1024,
            max_batch: 100,
            max_bytes: 1_000_000,
            idle_deadline_ms: 30_000,
            max_subscribers: 100,
        };
        let json = serde_json::to_string(&limits).unwrap();
        let decoded: SubscriberLimits = serde_json::from_str(&json).unwrap();
        assert_eq!(limits, decoded);
    }

    #[test]
    fn gap_event_round_trips() {
        let event = ControlledMediaEvent {
            header: header(),
            payload: ControlledEventPayload::Gap(EventGap {
                requested_sequence: EventSequence(1),
                first_available_sequence: EventSequence(100),
                instance_epoch: MediaNodeInstanceEpoch(42),
                reconciliation_required: true,
            }),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: ControlledMediaEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
        assert!(event.resource_ref().is_none());
    }
}
