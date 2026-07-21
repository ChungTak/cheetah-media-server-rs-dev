//! Durable event stream poll with resume cursor and gap detection (EVT-04/05).
//!
//! 可重放事件流：resume cursor、gap 检测与至少一次投递。

use std::sync::Arc;

use cheetah_media_api::controlled_event::{
    EventGap, EventSequence, EventSubscribeRequest, EventSubscribeResponse, SubscriberLimits,
};
use cheetah_media_api::cursor::OpaqueCursor;
use cheetah_media_api::error::{MediaError, MediaErrorCode};
use cheetah_media_api::event_cursor::{EventCursorCodec, EventCursorContents};
use cheetah_media_api::ids::{MediaNodeId, MediaNodeInstanceEpoch, TenantId};

use crate::error::ControlPlaneError;
use crate::event_store::{EventRecord, EventStore};
use crate::node_supervisor::Clock;
use crate::store::now_ms;

/// Default per-subscriber limits (EVT-06).
pub fn default_subscriber_limits() -> SubscriberLimits {
    SubscriberLimits {
        queue_capacity: 256,
        max_batch: 64,
        max_bytes: 1024 * 1024,
        idle_deadline_ms: 60_000,
        max_subscribers: 128,
    }
}

/// One poll page of the durable event journal.
///
/// 一次 poll 返回的 journal 页。
#[derive(Debug, Clone)]
pub struct EventPollPage {
    pub records: Vec<EventRecord>,
    /// Present when the resume sequence was below the retention floor.
    pub gap: Option<EventGap>,
    pub next_cursor: OpaqueCursor,
    pub response: EventSubscribeResponse,
}

/// Runtime-neutral durable event stream reader.
///
/// Does not open a long-lived gRPC stream; the gRPC adapter calls `poll` in a
/// loop and maps records to wire events. This keeps prost/tonic out of the
/// control-plane crate.
///
/// 运行时无关的可重放事件流读取器。
pub struct EventStreamService {
    events: Arc<dyn EventStore>,
    cursor_hmac_key: Vec<u8>,
    node_id: MediaNodeId,
    instance_epoch: MediaNodeInstanceEpoch,
    clock: Arc<dyn Clock>,
    limits: SubscriberLimits,
    cursor_ttl_ms: i64,
}

impl EventStreamService {
    pub fn new(
        events: Arc<dyn EventStore>,
        cursor_hmac_key: Vec<u8>,
        node_id: MediaNodeId,
        instance_epoch: MediaNodeInstanceEpoch,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ControlPlaneError> {
        if cursor_hmac_key.len() < 32 {
            return Err(ControlPlaneError::InvalidArgument(
                "event cursor HMAC key must be at least 32 bytes".to_string(),
            ));
        }
        Ok(Self {
            events,
            cursor_hmac_key,
            node_id,
            instance_epoch,
            clock,
            limits: default_subscriber_limits(),
            cursor_ttl_ms: 24 * 60 * 60 * 1000,
        })
    }

    pub fn with_limits(mut self, limits: SubscriberLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Poll the journal for the next page after the request's resume cursor.
    ///
    /// Semantics (EVT-04/05):
    /// 1. Validate tenant/filter and resume cursor HMAC/epoch/expiry.
    /// 2. If resume is older than retention floor, emit a single Gap and continue
    ///    from `first_available_sequence`.
    /// 3. Return up to `max_batch` non-expired events and a signed next cursor.
    pub async fn poll(
        &self,
        request: &EventSubscribeRequest,
    ) -> Result<EventPollPage, ControlPlaneError> {
        request
            .filter
            .validate()
            .map_err(ControlPlaneError::Media)?;
        if request.tenant_id != request.filter.tenant_id {
            return Err(ControlPlaneError::InvalidArgument(
                "subscribe tenant_id must match filter.tenant_id".to_string(),
            ));
        }

        let now = self.clock.now_ms();
        let max_batch = request.max_batch.min(self.limits.max_batch).max(1);

        let (mut start_seq, filter_digest) = self.decode_resume(request, now)?;

        let first = self
            .events
            .first_available_sequence(self.instance_epoch, now)
            .await?;

        let mut gap = None;
        if let Some(first_seq) = first {
            // Resume is exclusive (`sequence > start`); if the next expected
            // sequence is below the retention floor, report a gap.
            let next_expected = start_seq.0.saturating_add(1);
            if next_expected < first_seq.0 {
                gap = Some(EventGap {
                    requested_sequence: EventSequence(next_expected),
                    first_available_sequence: first_seq,
                    instance_epoch: self.instance_epoch,
                    reconciliation_required: true,
                });
                // Continue from just before first available so list_after yields it.
                start_seq = EventSequence(first_seq.0.saturating_sub(1));
            }
        }

        let mut records = self
            .events
            .list_after_sequence(self.instance_epoch, start_seq, max_batch)
            .await?;

        // Tenant / handle filters: journal may contain multiple tenants.
        records.retain(|r| r.tenant_id == request.tenant_id);
        if let Some(ref handle) = request.filter.resource_handle {
            records.retain(|r| r.resource_handle.as_ref() == Some(handle));
        }

        let last_seq = records.last().map(|r| r.sequence).unwrap_or(start_seq);

        let next_cursor = self.encode_cursor(last_seq, &filter_digest, now)?;

        // Domain event mapping is done by the gRPC adapter; this layer returns
        // an empty typed events list and the durable records for the adapter.
        let response = EventSubscribeResponse {
            events: Vec::new(),
            next_cursor: Some(next_cursor.clone()),
        };

        Ok(EventPollPage {
            records,
            gap,
            next_cursor,
            response,
        })
    }

    /// Purge expired journal rows (retention enforcement).
    pub async fn purge_expired(&self, max_rows: u32) -> Result<u64, ControlPlaneError> {
        self.events
            .purge_expired(self.clock.now_ms(), max_rows)
            .await
    }

    fn decode_resume(
        &self,
        request: &EventSubscribeRequest,
        now: i64,
    ) -> Result<(EventSequence, String), ControlPlaneError> {
        let filter_digest = request.filter.digest();
        let Some(cursor) = &request.resume_cursor else {
            return Ok((EventSequence(0), filter_digest));
        };

        let contents =
            EventCursorCodec::decode(cursor, &self.cursor_hmac_key, now, self.instance_epoch)
                .map_err(ControlPlaneError::Media)?;

        if contents.media_node_id != self.node_id {
            return Err(ControlPlaneError::Media(MediaError::new(
                MediaErrorCode::StaleOwner,
                "event cursor node id does not match this process",
            )));
        }
        if contents.tenant_filter_digest != filter_digest {
            return Err(ControlPlaneError::InvalidArgument(
                "event cursor filter digest does not match subscribe filter".to_string(),
            ));
        }

        Ok((contents.last_delivered_sequence, filter_digest))
    }

    fn encode_cursor(
        &self,
        last_seq: EventSequence,
        filter_digest: &str,
        now: i64,
    ) -> Result<OpaqueCursor, ControlPlaneError> {
        let contents = EventCursorContents {
            schema_version: EventCursorContents::CURRENT_SCHEMA_VERSION,
            media_node_id: self.node_id.clone(),
            media_node_instance_epoch: self.instance_epoch,
            last_delivered_sequence: last_seq,
            tenant_filter_digest: filter_digest.to_string(),
            issued_at_ms: now,
            expires_at_ms: now.saturating_add(self.cursor_ttl_ms),
            key_id: "k1".to_string(),
        };
        EventCursorCodec::encode(&contents, &self.cursor_hmac_key).map_err(ControlPlaneError::Media)
    }
}

/// Build a synthetic gap event id for logging/metrics only.
#[allow(dead_code)]
pub fn gap_metric_label(tenant: &TenantId) -> String {
    format!("gap:{}", tenant.as_str())
}

/// Helper used by tests that need a wall clock without FakeClock.
pub struct WallClock;

impl Clock for WallClock {
    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_supervisor::FakeClock;
    use crate::sqlite::SqliteStore;
    use cheetah_media_api::controlled_event::EventId;
    use cheetah_media_api::ids::MediaNodeId;
    use cheetah_media_api::resource_filter::ResourceFilter;
    use cheetah_runtime_tokio::TokioRuntime;

    fn key() -> Vec<u8> {
        vec![7u8; 32]
    }

    fn node() -> MediaNodeId {
        MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    fn sample(tenant: &TenantId, seq: u64, expires_at: i64) -> EventRecord {
        EventRecord {
            event_id: EventId::new(format!("event-{seq:04}")).unwrap(),
            instance_epoch: MediaNodeInstanceEpoch(1),
            sequence: EventSequence(seq),
            tenant_id: tenant.clone(),
            resource_kind: Some("publisher".to_string()),
            resource_handle: Some("pub-1".to_string()),
            occurred_at: 1000 + seq as i64,
            event_kind: "resource_state_changed".to_string(),
            serialized_payload: r#"{"state":"active"}"#.to_string(),
            correlation_id: None,
            traceparent: None,
            tracestate: None,
            // expires_at: 0 means never expire; otherwise absolute UTC ms.
            expires_at,
        }
    }

    async fn service(store: SqliteStore, clock: Arc<FakeClock>) -> EventStreamService {
        EventStreamService::new(
            Arc::new(store),
            key(),
            node(),
            MediaNodeInstanceEpoch(1),
            clock,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn poll_from_start_returns_events_and_cursor() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();
        let tenant = TenantId::new("tenant-1").unwrap();
        for seq in 1..=3 {
            store.append(&sample(&tenant, seq, 0)).await.unwrap();
        }
        let clock = Arc::new(FakeClock::new(1_000));
        let svc = service(store, clock).await;

        let req = EventSubscribeRequest {
            tenant_id: tenant.clone(),
            filter: ResourceFilter {
                tenant_id: tenant.clone(),
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
            max_batch: 10,
            max_bytes: 1024 * 1024,
        };
        let page = svc.poll(&req).await.unwrap();
        assert!(page.gap.is_none());
        assert_eq!(page.records.len(), 3);
        assert_eq!(page.records[0].sequence.0, 1);
        assert_eq!(page.records[2].sequence.0, 3);

        // Resume from next cursor should yield empty (caught up).
        let req2 = EventSubscribeRequest {
            resume_cursor: Some(page.next_cursor),
            ..req
        };
        let page2 = svc.poll(&req2).await.unwrap();
        assert!(page2.records.is_empty());
        assert!(page2.gap.is_none());
    }

    #[tokio::test]
    async fn poll_emits_gap_when_resume_below_retention() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();
        let tenant = TenantId::new("tenant-1").unwrap();
        // Only sequences 5..7 remain (1..4 "expired"/never written).
        for seq in 5..=7 {
            store.append(&sample(&tenant, seq, 0)).await.unwrap();
        }
        let clock = Arc::new(FakeClock::new(1_000));
        let svc = service(store, clock).await;

        // Craft a cursor that claims last delivered was sequence 1.
        let contents = EventCursorContents {
            schema_version: EventCursorContents::CURRENT_SCHEMA_VERSION,
            media_node_id: node(),
            media_node_instance_epoch: MediaNodeInstanceEpoch(1),
            last_delivered_sequence: EventSequence(1),
            tenant_filter_digest: ResourceFilter {
                tenant_id: tenant.clone(),
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
            }
            .digest(),
            issued_at_ms: 1_000,
            expires_at_ms: 1_000 + 60_000,
            key_id: "k1".to_string(),
        };
        let cursor = EventCursorCodec::encode(&contents, &key()).unwrap();

        let req = EventSubscribeRequest {
            tenant_id: tenant.clone(),
            filter: ResourceFilter {
                tenant_id: tenant.clone(),
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
            resume_cursor: Some(cursor),
            max_batch: 10,
            max_bytes: 1024 * 1024,
        };
        let page = svc.poll(&req).await.unwrap();
        let gap = page.gap.expect("gap required");
        assert!(gap.reconciliation_required);
        assert_eq!(gap.requested_sequence.0, 2);
        assert_eq!(gap.first_available_sequence.0, 5);
        assert_eq!(page.records[0].sequence.0, 5);
    }

    #[tokio::test]
    async fn purge_expired_removes_old_rows() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();
        let tenant = TenantId::new("tenant-1").unwrap();
        store.append(&sample(&tenant, 1, 500)).await.unwrap(); // expires at 500
        store.append(&sample(&tenant, 2, 0)).await.unwrap(); // never
        let clock = Arc::new(FakeClock::new(1_000));
        let svc = service(store.clone(), clock).await;
        let deleted = svc.purge_expired(100).await.unwrap();
        assert_eq!(deleted, 1);
        let first = store
            .first_available_sequence(MediaNodeInstanceEpoch(1), 1_000)
            .await
            .unwrap();
        assert_eq!(first, Some(EventSequence(2)));
    }
}
