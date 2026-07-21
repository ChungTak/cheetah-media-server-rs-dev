//! SQLite-backed `OrphanStore` implementation.
//!
//! SQLite 持久化 orphan 标记实现。

use async_trait::async_trait;
use cheetah_media_api::fencing::ControlledResourceRef;
use cheetah_media_api::ids::TenantId;
use rusqlite::{params, OptionalExtension};

use crate::error::ControlPlaneError;
use crate::sqlite::SqliteStore;
use crate::store::{OrphanRecord, OrphanStore};

struct RowOrphan {
    tenant_id: String,
    resource_kind: String,
    resource_handle: String,
    resource_ref_json: String,
    marked_at_ms: i64,
    confirmed: i64,
    confirmed_at_ms: Option<i64>,
}

impl RowOrphan {
    fn into_record(self) -> Result<OrphanRecord, ControlPlaneError> {
        Ok(OrphanRecord {
            tenant_id: TenantId::new(self.tenant_id)
                .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?,
            resource_kind: self.resource_kind,
            resource_handle: self.resource_handle,
            resource_ref_json: self.resource_ref_json,
            marked_at_ms: self.marked_at_ms,
            confirmed: self.confirmed != 0,
            confirmed_at_ms: self.confirmed_at_ms,
        })
    }
}

#[async_trait]
impl OrphanStore for SqliteStore {
    async fn mark_orphan(
        &self,
        resource_ref: &ControlledResourceRef,
        now_ms: i64,
    ) -> Result<(), ControlPlaneError> {
        let resource_ref_json = serde_json::to_string(resource_ref)
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let tenant = resource_ref.tenant_id.as_str().to_string();
        let kind = resource_ref.resource_kind.clone();
        let handle = resource_ref.resource_handle.clone();

        self.with_conn("orphan_mark", move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO orphan_records
                 (tenant_id, resource_kind, resource_handle, resource_ref_json,
                  marked_at_ms, confirmed, confirmed_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL)",
                params![tenant, kind, handle, resource_ref_json, now_ms],
            )?;
            Ok(())
        })
        .await
    }

    async fn get_orphan(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
    ) -> Result<Option<OrphanRecord>, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();

        self.with_conn("orphan_get", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, resource_kind, resource_handle, resource_ref_json,
                        marked_at_ms, confirmed, confirmed_at_ms
                 FROM orphan_records
                 WHERE tenant_id = ?1 AND resource_kind = ?2 AND resource_handle = ?3",
            )?;
            let row: Option<RowOrphan> = stmt
                .query_row(params![tenant, kind, handle], |row| {
                    Ok(RowOrphan {
                        tenant_id: row.get(0)?,
                        resource_kind: row.get(1)?,
                        resource_handle: row.get(2)?,
                        resource_ref_json: row.get(3)?,
                        marked_at_ms: row.get(4)?,
                        confirmed: row.get(5)?,
                        confirmed_at_ms: row.get(6)?,
                    })
                })
                .optional()?;
            row.map(|r| r.into_record()).transpose()
        })
        .await
    }

    async fn confirm_orphan(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        now_ms: i64,
    ) -> Result<(), ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();

        self.with_conn("orphan_confirm", move |conn| {
            conn.execute(
                "UPDATE orphan_records
                 SET confirmed = 1, confirmed_at_ms = ?4
                 WHERE tenant_id = ?1 AND resource_kind = ?2 AND resource_handle = ?3",
                params![tenant, kind, handle, now_ms],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_unconfirmed(
        &self,
        max_records: u32,
    ) -> Result<Vec<OrphanRecord>, ControlPlaneError> {
        self.with_conn("orphan_list_unconfirmed", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, resource_kind, resource_handle, resource_ref_json,
                        marked_at_ms, confirmed, confirmed_at_ms
                 FROM orphan_records
                 WHERE confirmed = 0
                 ORDER BY marked_at_ms ASC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![max_records as i64], |row| {
                Ok(RowOrphan {
                    tenant_id: row.get(0)?,
                    resource_kind: row.get(1)?,
                    resource_handle: row.get(2)?,
                    resource_ref_json: row.get(3)?,
                    marked_at_ms: row.get(4)?,
                    confirmed: row.get(5)?,
                    confirmed_at_ms: row.get(6)?,
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

    async fn list_unconfirmed_older_than(
        &self,
        before_ms: i64,
        max_records: u32,
    ) -> Result<Vec<OrphanRecord>, ControlPlaneError> {
        self.with_conn("orphan_list_unconfirmed_older_than", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, resource_kind, resource_handle, resource_ref_json,
                        marked_at_ms, confirmed, confirmed_at_ms
                 FROM orphan_records
                 WHERE confirmed = 0 AND marked_at_ms <= ?1
                 ORDER BY marked_at_ms ASC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![before_ms, max_records as i64], |row| {
                Ok(RowOrphan {
                    tenant_id: row.get(0)?,
                    resource_kind: row.get(1)?,
                    resource_handle: row.get(2)?,
                    resource_ref_json: row.get(3)?,
                    marked_at_ms: row.get(4)?,
                    confirmed: row.get(5)?,
                    confirmed_at_ms: row.get(6)?,
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

    async fn remove_orphan(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
    ) -> Result<(), ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();

        self.with_conn("orphan_remove", move |conn| {
            conn.execute(
                "DELETE FROM orphan_records
                 WHERE tenant_id = ?1 AND resource_kind = ?2 AND resource_handle = ?3",
                params![tenant, kind, handle],
            )?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cheetah_media_api::fencing::{ControlledResourceRef, ResourceOrigin};
    use cheetah_media_api::ids::{
        MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId, MediaSessionId, OwnerEpoch,
        ResourceGeneration, TenantId,
    };
    use cheetah_media_api::resource_filter::ResourceState;
    use cheetah_runtime_tokio::TokioRuntime;

    use super::*;
    use crate::store::{now_ms, ResourceRecord};

    fn runtime() -> Arc<TokioRuntime> {
        Arc::new(TokioRuntime::new())
    }

    async fn sqlite() -> SqliteStore {
        SqliteStore::new(runtime(), ":memory:").await.unwrap()
    }

    fn sample_resource(tenant: &TenantId) -> ResourceRecord {
        ResourceRecord {
            tenant_id: tenant.clone(),
            resource_kind: "publisher".to_string(),
            resource_handle: "pub-1".to_string(),
            media_session_id: Some(
                MediaSessionId::new("550e8400-e29b-41d4-a716-446655440001").unwrap(),
            ),
            media_binding_id: None,
            media_key: None,
            idempotency_scope: None,
            canonical_digest: None,
            accepted_owner_epoch: OwnerEpoch(1),
            media_node_id: Some(MediaNodeId::new("550e8400-e29b-41d4-a716-446655440003").unwrap()),
            media_node_instance_id: Some(
                MediaNodeInstanceId::new("550e8400-e29b-41d4-a716-446655440004").unwrap(),
            ),
            media_node_instance_epoch: MediaNodeInstanceEpoch(10),
            generation: ResourceGeneration(0),
            state: ResourceState::Active,
            safe_last_error: None,
            origin: ResourceOrigin::Cluster,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            terminal_at_ms: None,
        }
    }

    fn resource_ref(record: &ResourceRecord) -> ControlledResourceRef {
        ControlledResourceRef {
            tenant_id: record.tenant_id.clone(),
            media_session_id: record.media_session_id.clone(),
            media_binding_id: record.media_binding_id.clone(),
            resource_kind: record.resource_kind.clone(),
            resource_handle: record.resource_handle.clone(),
            owner_epoch: record.accepted_owner_epoch,
            node_instance_epoch: record.media_node_instance_epoch,
            generation: record.generation,
            origin: ResourceOrigin::default(),
        }
    }

    #[tokio::test]
    async fn orphan_mark_and_get_round_trip() {
        let store = sqlite().await;
        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_resource(&tenant);
        let resource_ref = resource_ref(&record);

        store.mark_orphan(&resource_ref, 1_000).await.unwrap();
        let orphan = store
            .get_orphan(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .unwrap();
        assert!(!orphan.confirmed);
        assert_eq!(orphan.marked_at_ms, 1_000);
    }

    #[tokio::test]
    async fn orphan_confirm_and_list_older_than() {
        let store = sqlite().await;
        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_resource(&tenant);
        let resource_ref = resource_ref(&record);

        store.mark_orphan(&resource_ref, 1_000).await.unwrap();
        store
            .confirm_orphan(&tenant, "publisher", "pub-1", 2_000)
            .await
            .unwrap();

        let orphan = store
            .get_orphan(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .unwrap();
        assert!(orphan.confirmed);
        assert_eq!(orphan.confirmed_at_ms, Some(2_000));
    }

    #[tokio::test]
    async fn orphan_list_unconfirmed_older_than_filters_by_marked_at() {
        let store = sqlite().await;
        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_resource(&tenant);
        let resource_ref = resource_ref(&record);

        store.mark_orphan(&resource_ref, 1_000).await.unwrap();
        let older = store.list_unconfirmed_older_than(2_000, 10).await.unwrap();
        assert_eq!(older.len(), 1);

        let newer = store.list_unconfirmed_older_than(500, 10).await.unwrap();
        assert!(newer.is_empty());
    }

    #[tokio::test]
    async fn orphan_remove_deletes_mark() {
        let store = sqlite().await;
        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_resource(&tenant);
        let resource_ref = resource_ref(&record);

        store.mark_orphan(&resource_ref, 1_000).await.unwrap();
        store
            .remove_orphan(&tenant, "publisher", "pub-1")
            .await
            .unwrap();
        assert!(store
            .get_orphan(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .is_none());
    }
}
