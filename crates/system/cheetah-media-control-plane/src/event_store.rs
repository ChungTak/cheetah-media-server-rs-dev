//! SQLite-backed durable event journal.
//!
//! 基于 SQLite 的可重放事件日志存储。

use async_trait::async_trait;
use cheetah_media_api::controlled_event::{EventId, EventSequence};
use cheetah_media_api::ids::{MediaNodeInstanceEpoch, TenantId};
use rusqlite::{params, OptionalExtension};

use crate::error::ControlPlaneError;
use crate::sqlite::SqliteStore;

/// A durable event journal record.
///
/// 持久化事件日志记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub event_id: EventId,
    pub instance_epoch: MediaNodeInstanceEpoch,
    pub sequence: EventSequence,
    pub tenant_id: TenantId,
    pub resource_kind: Option<String>,
    pub resource_handle: Option<String>,
    pub occurred_at: i64,
    pub event_kind: String,
    pub serialized_payload: String,
    pub correlation_id: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
    pub expires_at: i64,
}

/// Durable append-only event storage.
///
/// 持久化仅追加事件存储。
#[async_trait]
pub trait EventStore: Send + Sync {
    /// Append a single event to the journal.
    ///
    /// The caller is responsible for assigning a unique `event_id` and
    /// monotonic `sequence` within the `instance_epoch`.
    async fn append(&self, record: &EventRecord) -> Result<(), ControlPlaneError>;

    /// Fetch an event by its `(instance_epoch, sequence)` key.
    async fn get_by_sequence(
        &self,
        instance_epoch: MediaNodeInstanceEpoch,
        sequence: EventSequence,
    ) -> Result<Option<EventRecord>, ControlPlaneError>;

    /// List events for a tenant starting after `(start_epoch, start_sequence)`.
    ///
    /// The cursor is ordered by `(instance_epoch, sequence)` so events from
    /// different node instance lifetimes do not interleave or duplicate.
    async fn list_by_tenant(
        &self,
        tenant_id: &TenantId,
        start_epoch: MediaNodeInstanceEpoch,
        start_sequence: EventSequence,
        limit: u32,
    ) -> Result<Vec<EventRecord>, ControlPlaneError>;

    /// List events for a specific resource starting after
    /// `(start_epoch, start_sequence)`.
    async fn list_by_resource(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        start_epoch: MediaNodeInstanceEpoch,
        start_sequence: EventSequence,
        limit: u32,
    ) -> Result<Vec<EventRecord>, ControlPlaneError>;

    /// List events after a sequence within a single node instance epoch.
    async fn list_after_sequence(
        &self,
        instance_epoch: MediaNodeInstanceEpoch,
        start_sequence: EventSequence,
        limit: u32,
    ) -> Result<Vec<EventRecord>, ControlPlaneError>;

    /// Return the next monotonic sequence number for `instance_epoch`.
    async fn next_sequence(
        &self,
        instance_epoch: MediaNodeInstanceEpoch,
    ) -> Result<EventSequence, ControlPlaneError>;
}

struct RowEvent {
    event_id: String,
    instance_epoch: i64,
    sequence: i64,
    tenant_id: String,
    resource_kind: Option<String>,
    resource_handle: Option<String>,
    occurred_at: i64,
    event_kind: String,
    serialized_payload: String,
    correlation_id: Option<String>,
    traceparent: Option<String>,
    tracestate: Option<String>,
    expires_at: i64,
}

impl RowEvent {
    fn into_record(self) -> Result<EventRecord, ControlPlaneError> {
        let event_id = EventId::new(self.event_id)
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let tenant_id = TenantId::new(self.tenant_id)
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        Ok(EventRecord {
            event_id,
            instance_epoch: MediaNodeInstanceEpoch(self.instance_epoch as u64),
            sequence: EventSequence(self.sequence as u64),
            tenant_id,
            resource_kind: self.resource_kind,
            resource_handle: self.resource_handle,
            occurred_at: self.occurred_at,
            event_kind: self.event_kind,
            serialized_payload: self.serialized_payload,
            correlation_id: self.correlation_id,
            traceparent: self.traceparent,
            tracestate: self.tracestate,
            expires_at: self.expires_at,
        })
    }
}

#[async_trait]
impl EventStore for SqliteStore {
    async fn append(&self, record: &EventRecord) -> Result<(), ControlPlaneError> {
        let record = record.clone();
        self.with_conn("event_append", move |conn| {
            conn.execute(
                "INSERT INTO media_events
                 (instance_epoch, sequence, event_id, tenant_id, resource_kind,
                  resource_handle, occurred_at, event_kind, serialized_payload,
                  correlation_id, traceparent, tracestate, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    record.instance_epoch.0 as i64,
                    record.sequence.0 as i64,
                    record.event_id.as_str(),
                    record.tenant_id.as_str(),
                    record.resource_kind,
                    record.resource_handle,
                    record.occurred_at,
                    record.event_kind,
                    record.serialized_payload,
                    record.correlation_id,
                    record.traceparent,
                    record.tracestate,
                    record.expires_at,
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn get_by_sequence(
        &self,
        instance_epoch: MediaNodeInstanceEpoch,
        sequence: EventSequence,
    ) -> Result<Option<EventRecord>, ControlPlaneError> {
        let epoch = instance_epoch.0 as i64;
        let seq = sequence.0 as i64;
        self.with_conn("event_get_by_sequence", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT instance_epoch, sequence, event_id, tenant_id, resource_kind,
                        resource_handle, occurred_at, event_kind, serialized_payload,
                        correlation_id, traceparent, tracestate, expires_at
                 FROM media_events
                 WHERE instance_epoch = ?1 AND sequence = ?2",
            )?;
            let row: Option<RowEvent> = stmt
                .query_row(params![epoch, seq], |row| {
                    Ok(RowEvent {
                        instance_epoch: row.get(0)?,
                        sequence: row.get(1)?,
                        event_id: row.get(2)?,
                        tenant_id: row.get(3)?,
                        resource_kind: row.get(4)?,
                        resource_handle: row.get(5)?,
                        occurred_at: row.get(6)?,
                        event_kind: row.get(7)?,
                        serialized_payload: row.get(8)?,
                        correlation_id: row.get(9)?,
                        traceparent: row.get(10)?,
                        tracestate: row.get(11)?,
                        expires_at: row.get(12)?,
                    })
                })
                .optional()?;
            row.map(|r| r.into_record()).transpose()
        })
        .await
    }

    async fn list_by_tenant(
        &self,
        tenant_id: &TenantId,
        start_epoch: MediaNodeInstanceEpoch,
        start_sequence: EventSequence,
        limit: u32,
    ) -> Result<Vec<EventRecord>, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let start_epoch_i64 = start_epoch.0 as i64;
        let start = start_sequence.0 as i64;
        let limit = limit as i64;
        self.with_conn("event_list_by_tenant", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT instance_epoch, sequence, event_id, tenant_id, resource_kind,
                        resource_handle, occurred_at, event_kind, serialized_payload,
                        correlation_id, traceparent, tracestate, expires_at
                 FROM media_events
                 WHERE tenant_id = ?1
                   AND (instance_epoch > ?2 OR (instance_epoch = ?3 AND sequence > ?4))
                 ORDER BY instance_epoch ASC, sequence ASC
                 LIMIT ?5",
            )?;
            let rows = stmt.query_map(
                params![tenant, start_epoch_i64, start_epoch_i64, start, limit],
                |row| {
                    Ok(RowEvent {
                        instance_epoch: row.get(0)?,
                        sequence: row.get(1)?,
                        event_id: row.get(2)?,
                        tenant_id: row.get(3)?,
                        resource_kind: row.get(4)?,
                        resource_handle: row.get(5)?,
                        occurred_at: row.get(6)?,
                        event_kind: row.get(7)?,
                        serialized_payload: row.get(8)?,
                        correlation_id: row.get(9)?,
                        traceparent: row.get(10)?,
                        tracestate: row.get(11)?,
                        expires_at: row.get(12)?,
                    })
                },
            )?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row?.into_record()?);
            }
            Ok(records)
        })
        .await
    }

    async fn list_by_resource(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        start_epoch: MediaNodeInstanceEpoch,
        start_sequence: EventSequence,
        limit: u32,
    ) -> Result<Vec<EventRecord>, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();
        let start_epoch_i64 = start_epoch.0 as i64;
        let start = start_sequence.0 as i64;
        let limit = limit as i64;
        self.with_conn("event_list_by_resource", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT instance_epoch, sequence, event_id, tenant_id, resource_kind,
                        resource_handle, occurred_at, event_kind, serialized_payload,
                        correlation_id, traceparent, tracestate, expires_at
                 FROM media_events
                 WHERE tenant_id = ?1 AND resource_kind = ?2 AND resource_handle = ?3
                   AND (instance_epoch > ?4 OR (instance_epoch = ?5 AND sequence > ?6))
                 ORDER BY instance_epoch ASC, sequence ASC
                 LIMIT ?7",
            )?;
            let rows = stmt.query_map(
                params![
                    tenant,
                    kind,
                    handle,
                    start_epoch_i64,
                    start_epoch_i64,
                    start,
                    limit
                ],
                |row| {
                    Ok(RowEvent {
                        instance_epoch: row.get(0)?,
                        sequence: row.get(1)?,
                        event_id: row.get(2)?,
                        tenant_id: row.get(3)?,
                        resource_kind: row.get(4)?,
                        resource_handle: row.get(5)?,
                        occurred_at: row.get(6)?,
                        event_kind: row.get(7)?,
                        serialized_payload: row.get(8)?,
                        correlation_id: row.get(9)?,
                        traceparent: row.get(10)?,
                        tracestate: row.get(11)?,
                        expires_at: row.get(12)?,
                    })
                },
            )?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row?.into_record()?);
            }
            Ok(records)
        })
        .await
    }

    async fn list_after_sequence(
        &self,
        instance_epoch: MediaNodeInstanceEpoch,
        start_sequence: EventSequence,
        limit: u32,
    ) -> Result<Vec<EventRecord>, ControlPlaneError> {
        let epoch = instance_epoch.0 as i64;
        let start = start_sequence.0 as i64;
        let limit = limit as i64;
        self.with_conn("event_list_after_sequence", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT instance_epoch, sequence, event_id, tenant_id, resource_kind,
                        resource_handle, occurred_at, event_kind, serialized_payload,
                        correlation_id, traceparent, tracestate, expires_at
                 FROM media_events
                 WHERE instance_epoch = ?1 AND sequence > ?2
                 ORDER BY sequence ASC
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![epoch, start, limit], |row| {
                Ok(RowEvent {
                    instance_epoch: row.get(0)?,
                    sequence: row.get(1)?,
                    event_id: row.get(2)?,
                    tenant_id: row.get(3)?,
                    resource_kind: row.get(4)?,
                    resource_handle: row.get(5)?,
                    occurred_at: row.get(6)?,
                    event_kind: row.get(7)?,
                    serialized_payload: row.get(8)?,
                    correlation_id: row.get(9)?,
                    traceparent: row.get(10)?,
                    tracestate: row.get(11)?,
                    expires_at: row.get(12)?,
                })
            })?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row?.into_record()?);
            }
            Ok(records)
        })
        .await
    }

    async fn next_sequence(
        &self,
        instance_epoch: MediaNodeInstanceEpoch,
    ) -> Result<EventSequence, ControlPlaneError> {
        let epoch = instance_epoch.0 as i64;
        self.with_conn("event_next_sequence", move |conn| {
            let next: i64 = conn.query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM media_events
                 WHERE instance_epoch = ?1",
                params![epoch],
                |row| row.get(0),
            )?;
            Ok(EventSequence(next as u64))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cheetah_media_api::ids::TenantId;
    use cheetah_runtime_tokio::TokioRuntime;

    use super::*;

    fn sample_event(tenant: &TenantId, seq: u64) -> EventRecord {
        EventRecord {
            event_id: EventId::new(format!("event-{seq:04}")).unwrap(),
            instance_epoch: MediaNodeInstanceEpoch(7),
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
            expires_at: 9999999,
        }
    }

    #[tokio::test]
    async fn event_append_and_get_round_trip() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let event = sample_event(&tenant, 1);
        store.append(&event).await.unwrap();

        let loaded = store
            .get_by_sequence(event.instance_epoch, event.sequence)
            .await
            .unwrap()
            .expect("event exists");
        assert_eq!(loaded.event_id, event.event_id);
        assert_eq!(loaded.sequence, event.sequence);
        assert_eq!(loaded.serialized_payload, event.serialized_payload);
    }

    #[tokio::test]
    async fn event_list_by_tenant_and_resource() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        for seq in 1..=3 {
            store.append(&sample_event(&tenant, seq)).await.unwrap();
        }

        let by_tenant = store
            .list_by_tenant(&tenant, MediaNodeInstanceEpoch(0), EventSequence(0), 10)
            .await
            .unwrap();
        assert_eq!(by_tenant.len(), 3);

        let by_resource = store
            .list_by_resource(
                &tenant,
                "publisher",
                "pub-1",
                MediaNodeInstanceEpoch(0),
                EventSequence(0),
                10,
            )
            .await
            .unwrap();
        assert_eq!(by_resource.len(), 3);

        let after = store
            .list_after_sequence(MediaNodeInstanceEpoch(7), EventSequence(1), 10)
            .await
            .unwrap();
        assert_eq!(after.len(), 2);
    }
}
