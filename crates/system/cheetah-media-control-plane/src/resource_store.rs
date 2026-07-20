//! SQLite-backed `ResourceStore` implementation.
//!
//! SQLite 持久化受控资源存储实现。

use async_trait::async_trait;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::ids::{
    MediaBindingId, MediaKey, MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId,
    MediaSessionId, OwnerEpoch, ResourceGeneration, TenantId,
};
use cheetah_media_api::resource_filter::ResourceState;
use rusqlite::{params, OptionalExtension};

use crate::error::ControlPlaneError;
use crate::idempotency::CanonicalDigest;
use crate::sqlite::SqliteStore;
use crate::store::{now_ms, ResourceRecord, ResourceStore};

struct RowResource {
    tenant_id: String,
    resource_kind: String,
    resource_handle: String,
    media_session_id: Option<String>,
    media_binding_id: Option<String>,
    media_key: Option<String>,
    idempotency_scope: Option<String>,
    canonical_digest: Option<String>,
    accepted_owner_epoch: i64,
    media_node_id: Option<String>,
    media_node_instance_id: Option<String>,
    media_node_instance_epoch: i64,
    generation: i64,
    state: String,
    safe_last_error: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    terminal_at_ms: Option<i64>,
}

impl RowResource {
    fn into_record(self) -> Result<ResourceRecord, ControlPlaneError> {
        let tenant_id = TenantId::new(self.tenant_id)
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let media_session_id = self
            .media_session_id
            .map(MediaSessionId::new)
            .transpose()
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let media_binding_id = self
            .media_binding_id
            .map(MediaBindingId::new)
            .transpose()
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let media_node_id = self
            .media_node_id
            .map(MediaNodeId::new)
            .transpose()
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let media_node_instance_id = self
            .media_node_instance_id
            .map(MediaNodeInstanceId::new)
            .transpose()
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let media_key = self
            .media_key
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let canonical_digest = match self.canonical_digest {
            Some(hex) => Some(CanonicalDigest::from_hex(&hex).ok_or_else(|| {
                ControlPlaneError::Serialization("invalid canonical digest".to_string())
            })?),
            None => None,
        };
        let safe_last_error = self
            .safe_last_error
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;

        Ok(ResourceRecord {
            tenant_id,
            resource_kind: self.resource_kind,
            resource_handle: self.resource_handle,
            media_session_id,
            media_binding_id,
            media_key,
            idempotency_scope: self.idempotency_scope,
            canonical_digest,
            accepted_owner_epoch: OwnerEpoch(self.accepted_owner_epoch as u64),
            media_node_id,
            media_node_instance_id,
            media_node_instance_epoch: MediaNodeInstanceEpoch(
                self.media_node_instance_epoch as u64,
            ),
            generation: ResourceGeneration(self.generation as u64),
            state: str_to_state(&self.state),
            safe_last_error,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            terminal_at_ms: self.terminal_at_ms,
        })
    }
}

fn state_to_str(state: ResourceState) -> &'static str {
    match state {
        ResourceState::Pending => "pending",
        ResourceState::Active => "active",
        ResourceState::Stopping => "stopping",
        ResourceState::Stopped => "stopped",
        ResourceState::Failed => "failed",
    }
}

fn str_to_state(s: &str) -> ResourceState {
    match s {
        "active" => ResourceState::Active,
        "stopping" => ResourceState::Stopping,
        "stopped" => ResourceState::Stopped,
        "failed" => ResourceState::Failed,
        _ => ResourceState::Pending,
    }
}

fn encode_media_key(key: &MediaKey) -> Result<String, ControlPlaneError> {
    serde_json::to_string(key).map_err(|e| ControlPlaneError::Serialization(e.to_string()))
}

fn encode_media_error(error: &MediaError) -> Result<String, ControlPlaneError> {
    serde_json::to_string(error).map_err(|e| ControlPlaneError::Serialization(e.to_string()))
}

#[async_trait]
impl ResourceStore for SqliteStore {
    async fn get(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
    ) -> Result<Option<ResourceRecord>, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();
        self.with_conn("resource_get", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, resource_kind, resource_handle, media_session_id,
                        media_binding_id, media_key, idempotency_scope, canonical_digest,
                        accepted_owner_epoch, media_node_id, media_node_instance_id,
                        media_node_instance_epoch, generation, state, safe_last_error,
                        created_at_ms, updated_at_ms, terminal_at_ms
                 FROM controlled_resources
                 WHERE tenant_id = ?1 AND resource_kind = ?2 AND resource_handle = ?3",
            )?;
            let row: Option<RowResource> = stmt
                .query_row(params![tenant, kind, handle], |row| {
                    Ok(RowResource {
                        tenant_id: row.get(0)?,
                        resource_kind: row.get(1)?,
                        resource_handle: row.get(2)?,
                        media_session_id: row.get(3)?,
                        media_binding_id: row.get(4)?,
                        media_key: row.get(5)?,
                        idempotency_scope: row.get(6)?,
                        canonical_digest: row.get(7)?,
                        accepted_owner_epoch: row.get(8)?,
                        media_node_id: row.get(9)?,
                        media_node_instance_id: row.get(10)?,
                        media_node_instance_epoch: row.get(11)?,
                        generation: row.get(12)?,
                        state: row.get(13)?,
                        safe_last_error: row.get(14)?,
                        created_at_ms: row.get(15)?,
                        updated_at_ms: row.get(16)?,
                        terminal_at_ms: row.get(17)?,
                    })
                })
                .optional()?;
            row.map(|r| r.into_record()).transpose()
        })
        .await
    }

    async fn insert(&self, record: &ResourceRecord) -> Result<(), ControlPlaneError> {
        let record = record.clone();
        self.with_conn("resource_insert", move |conn| {
            let tx = conn.transaction()?;

            let exists: Option<String> = {
                let mut stmt = tx.prepare(
                    "SELECT tenant_id FROM controlled_resources
                     WHERE tenant_id = ?1 AND resource_kind = ?2 AND resource_handle = ?3",
                )?;
                stmt.query_row(
                    params![
                        record.tenant_id.as_str(),
                        record.resource_kind,
                        record.resource_handle,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            };
            if exists.is_some() {
                return Err(ControlPlaneError::Conflict(
                    "controlled resource already exists".to_string(),
                ));
            }

            do_insert(&tx, &record)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn compare_and_set_owner_epoch(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        expected: OwnerEpoch,
        new: OwnerEpoch,
    ) -> Result<bool, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();
        self.with_conn("resource_cas_owner_epoch", move |conn| {
            let updated = conn.execute(
                "UPDATE controlled_resources
                 SET accepted_owner_epoch = ?1, updated_at_ms = ?2
                 WHERE tenant_id = ?3 AND resource_kind = ?4 AND resource_handle = ?5
                   AND accepted_owner_epoch = ?6",
                params![
                    new.0 as i64,
                    now_ms(),
                    tenant,
                    kind,
                    handle,
                    expected.0 as i64,
                ],
            )?;
            Ok(updated == 1)
        })
        .await
    }

    async fn compare_and_set_generation(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        expected: ResourceGeneration,
        new: ResourceGeneration,
        state: ResourceState,
    ) -> Result<bool, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();
        self.with_conn("resource_cas_generation", move |conn| {
            let terminal_at = if state.is_terminal() {
                Some(now_ms())
            } else {
                None
            };
            let updated = conn.execute(
                "UPDATE controlled_resources
                 SET generation = ?1, state = ?2, terminal_at_ms = ?3, updated_at_ms = ?4
                 WHERE tenant_id = ?5 AND resource_kind = ?6 AND resource_handle = ?7
                   AND generation = ?8",
                params![
                    new.0 as i64,
                    state_to_str(state),
                    terminal_at,
                    now_ms(),
                    tenant,
                    kind,
                    handle,
                    expected.0 as i64,
                ],
            )?;
            Ok(updated == 1)
        })
        .await
    }

    async fn set_state(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        state: ResourceState,
    ) -> Result<(), ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let kind = resource_kind.to_string();
        let handle = resource_handle.to_string();
        self.with_conn("resource_set_state", move |conn| {
            let terminal_at = if state.is_terminal() {
                Some(now_ms())
            } else {
                None
            };
            conn.execute(
                "UPDATE controlled_resources
                 SET state = ?1, terminal_at_ms = ?2, updated_at_ms = ?3
                 WHERE tenant_id = ?4 AND resource_kind = ?5 AND resource_handle = ?6",
                params![
                    state_to_str(state),
                    terminal_at,
                    now_ms(),
                    tenant,
                    kind,
                    handle,
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn tombstone(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        state: ResourceState,
    ) -> Result<(), ControlPlaneError> {
        if !state.is_terminal() {
            return Err(ControlPlaneError::InvalidArgument(
                "tombstone state must be terminal".to_string(),
            ));
        }
        self.set_state(tenant_id, resource_kind, resource_handle, state)
            .await
    }

    async fn list_by_session(
        &self,
        tenant_id: &TenantId,
        session_id: &MediaSessionId,
    ) -> Result<Vec<ResourceRecord>, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let session = session_id.as_str().to_string();
        self.with_conn("resource_list_by_session", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, resource_kind, resource_handle, media_session_id,
                        media_binding_id, media_key, idempotency_scope, canonical_digest,
                        accepted_owner_epoch, media_node_id, media_node_instance_id,
                        media_node_instance_epoch, generation, state, safe_last_error,
                        created_at_ms, updated_at_ms, terminal_at_ms
                 FROM controlled_resources
                 WHERE tenant_id = ?1 AND media_session_id = ?2
                 ORDER BY updated_at_ms DESC",
            )?;
            let rows = stmt.query_map(params![tenant, session], |row| {
                Ok(RowResource {
                    tenant_id: row.get(0)?,
                    resource_kind: row.get(1)?,
                    resource_handle: row.get(2)?,
                    media_session_id: row.get(3)?,
                    media_binding_id: row.get(4)?,
                    media_key: row.get(5)?,
                    idempotency_scope: row.get(6)?,
                    canonical_digest: row.get(7)?,
                    accepted_owner_epoch: row.get(8)?,
                    media_node_id: row.get(9)?,
                    media_node_instance_id: row.get(10)?,
                    media_node_instance_epoch: row.get(11)?,
                    generation: row.get(12)?,
                    state: row.get(13)?,
                    safe_last_error: row.get(14)?,
                    created_at_ms: row.get(15)?,
                    updated_at_ms: row.get(16)?,
                    terminal_at_ms: row.get(17)?,
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

    async fn list_by_binding(
        &self,
        tenant_id: &TenantId,
        binding_id: &MediaBindingId,
    ) -> Result<Vec<ResourceRecord>, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let binding = binding_id.as_str().to_string();
        self.with_conn("resource_list_by_binding", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, resource_kind, resource_handle, media_session_id,
                        media_binding_id, media_key, idempotency_scope, canonical_digest,
                        accepted_owner_epoch, media_node_id, media_node_instance_id,
                        media_node_instance_epoch, generation, state, safe_last_error,
                        created_at_ms, updated_at_ms, terminal_at_ms
                 FROM controlled_resources
                 WHERE tenant_id = ?1 AND media_binding_id = ?2
                 ORDER BY updated_at_ms DESC",
            )?;
            let rows = stmt.query_map(params![tenant, binding], |row| {
                Ok(RowResource {
                    tenant_id: row.get(0)?,
                    resource_kind: row.get(1)?,
                    resource_handle: row.get(2)?,
                    media_session_id: row.get(3)?,
                    media_binding_id: row.get(4)?,
                    media_key: row.get(5)?,
                    idempotency_scope: row.get(6)?,
                    canonical_digest: row.get(7)?,
                    accepted_owner_epoch: row.get(8)?,
                    media_node_id: row.get(9)?,
                    media_node_instance_id: row.get(10)?,
                    media_node_instance_epoch: row.get(11)?,
                    generation: row.get(12)?,
                    state: row.get(13)?,
                    safe_last_error: row.get(14)?,
                    created_at_ms: row.get(15)?,
                    updated_at_ms: row.get(16)?,
                    terminal_at_ms: row.get(17)?,
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

    async fn list_by_node(
        &self,
        tenant_id: &TenantId,
        node_id: &MediaNodeId,
    ) -> Result<Vec<ResourceRecord>, ControlPlaneError> {
        let tenant = tenant_id.as_str().to_string();
        let node = node_id.as_str().to_string();
        self.with_conn("resource_list_by_node", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, resource_kind, resource_handle, media_session_id,
                        media_binding_id, media_key, idempotency_scope, canonical_digest,
                        accepted_owner_epoch, media_node_id, media_node_instance_id,
                        media_node_instance_epoch, generation, state, safe_last_error,
                        created_at_ms, updated_at_ms, terminal_at_ms
                 FROM controlled_resources
                 WHERE tenant_id = ?1 AND media_node_id = ?2
                 ORDER BY updated_at_ms DESC",
            )?;
            let rows = stmt.query_map(params![tenant, node], |row| {
                Ok(RowResource {
                    tenant_id: row.get(0)?,
                    resource_kind: row.get(1)?,
                    resource_handle: row.get(2)?,
                    media_session_id: row.get(3)?,
                    media_binding_id: row.get(4)?,
                    media_key: row.get(5)?,
                    idempotency_scope: row.get(6)?,
                    canonical_digest: row.get(7)?,
                    accepted_owner_epoch: row.get(8)?,
                    media_node_id: row.get(9)?,
                    media_node_instance_id: row.get(10)?,
                    media_node_instance_epoch: row.get(11)?,
                    generation: row.get(12)?,
                    state: row.get(13)?,
                    safe_last_error: row.get(14)?,
                    created_at_ms: row.get(15)?,
                    updated_at_ms: row.get(16)?,
                    terminal_at_ms: row.get(17)?,
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
}

fn do_insert(
    tx: &rusqlite::Transaction<'_>,
    record: &ResourceRecord,
) -> Result<(), ControlPlaneError> {
    let media_key_json = record
        .media_key
        .as_ref()
        .map(encode_media_key)
        .transpose()?;
    let canonical_digest_hex = record
        .canonical_digest
        .as_ref()
        .map(CanonicalDigest::to_hex);
    let safe_last_error_json = record
        .safe_last_error
        .as_ref()
        .map(encode_media_error)
        .transpose()?;
    let terminal_at_ms = if record.state.is_terminal() {
        Some(record.terminal_at_ms.unwrap_or(now_ms()))
    } else {
        None
    };

    tx.execute(
        "INSERT INTO controlled_resources
         (tenant_id, resource_kind, resource_handle, media_session_id, media_binding_id,
          media_key, idempotency_scope, canonical_digest, accepted_owner_epoch,
          media_node_id, media_node_instance_id, media_node_instance_epoch, generation,
          state, safe_last_error, created_at_ms, updated_at_ms, terminal_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            record.tenant_id.as_str(),
            record.resource_kind,
            record.resource_handle,
            record.media_session_id.as_ref().map(MediaSessionId::as_str),
            record.media_binding_id.as_ref().map(MediaBindingId::as_str),
            media_key_json,
            record.idempotency_scope.as_ref(),
            canonical_digest_hex,
            record.accepted_owner_epoch.0 as i64,
            record.media_node_id.as_ref().map(MediaNodeId::as_str),
            record
                .media_node_instance_id
                .as_ref()
                .map(MediaNodeInstanceId::as_str),
            record.media_node_instance_epoch.0 as i64,
            record.generation.0 as i64,
            state_to_str(record.state),
            safe_last_error_json,
            record.created_at_ms,
            record.updated_at_ms,
            terminal_at_ms,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cheetah_media_api::fencing::ControlledResourceRef;
    use cheetah_media_api::ids::{
        MediaBindingId, MediaKey, MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId,
        MediaSessionId, OwnerEpoch, ResourceGeneration, TenantId,
    };
    use cheetah_media_api::resource_filter::ResourceState;
    use cheetah_runtime_tokio::TokioRuntime;

    use super::*;

    fn sample_record(tenant: &TenantId) -> ResourceRecord {
        let session = MediaSessionId::new("550e8400-e29b-41d4-a716-446655440001").unwrap();
        let binding = MediaBindingId::new("550e8400-e29b-41d4-a716-446655440002").unwrap();
        let node = MediaNodeId::new("550e8400-e29b-41d4-a716-446655440003").unwrap();
        let node_instance =
            MediaNodeInstanceId::new("550e8400-e29b-41d4-a716-446655440004").unwrap();
        let media_key = MediaKey::new("__defaultVhost__", "live", "test", None).unwrap();

        ResourceRecord {
            tenant_id: tenant.clone(),
            resource_kind: "publisher".to_string(),
            resource_handle: "pub-1".to_string(),
            media_session_id: Some(session),
            media_binding_id: Some(binding),
            media_key: Some(media_key),
            idempotency_scope: Some("tenant-1/create/scope".to_string()),
            canonical_digest: None,
            accepted_owner_epoch: OwnerEpoch(1),
            media_node_id: Some(node),
            media_node_instance_id: Some(node_instance),
            media_node_instance_epoch: MediaNodeInstanceEpoch(10),
            generation: ResourceGeneration(0),
            state: ResourceState::Pending,
            safe_last_error: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            terminal_at_ms: None,
        }
    }

    #[tokio::test]
    async fn resource_insert_and_get_round_trip() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_record(&tenant);
        let expected_ref: ControlledResourceRef = record.resource_ref();

        store.insert(&record).await.unwrap();
        let loaded = store
            .get(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .expect("record exists");

        assert_eq!(loaded.tenant_id, tenant);
        assert_eq!(loaded.resource_handle, "pub-1");
        assert_eq!(loaded.state, ResourceState::Pending);
        assert_eq!(loaded.resource_ref(), expected_ref);
    }

    #[tokio::test]
    async fn resource_insert_conflict_on_duplicate_key() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_record(&tenant);

        store.insert(&record).await.unwrap();
        let err = store.insert(&record).await.unwrap_err();
        assert!(matches!(err, ControlPlaneError::Conflict(_)));
    }

    #[tokio::test]
    async fn resource_compare_and_set_owner_epoch() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_record(&tenant);
        store.insert(&record).await.unwrap();

        let ok = store
            .compare_and_set_owner_epoch(
                &tenant,
                "publisher",
                "pub-1",
                OwnerEpoch(1),
                OwnerEpoch(2),
            )
            .await
            .unwrap();
        assert!(ok);

        let ok = store
            .compare_and_set_owner_epoch(
                &tenant,
                "publisher",
                "pub-1",
                OwnerEpoch(1),
                OwnerEpoch(3),
            )
            .await
            .unwrap();
        assert!(!ok);

        let loaded = store
            .get(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.accepted_owner_epoch, OwnerEpoch(2));
    }

    #[tokio::test]
    async fn resource_compare_and_set_generation() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let mut record = sample_record(&tenant);
        record.state = ResourceState::Active;
        store.insert(&record).await.unwrap();

        let ok = store
            .compare_and_set_generation(
                &tenant,
                "publisher",
                "pub-1",
                ResourceGeneration(0),
                ResourceGeneration(1),
                ResourceState::Active,
            )
            .await
            .unwrap();
        assert!(ok);

        let loaded = store
            .get(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.generation, ResourceGeneration(1));
        assert_eq!(loaded.state, ResourceState::Active);
    }

    #[tokio::test]
    async fn resource_set_state_records_terminal_at() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_record(&tenant);
        store.insert(&record).await.unwrap();

        store
            .set_state(&tenant, "publisher", "pub-1", ResourceState::Active)
            .await
            .unwrap();

        store
            .set_state(&tenant, "publisher", "pub-1", ResourceState::Stopped)
            .await
            .unwrap();

        let loaded = store
            .get(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ResourceState::Stopped);
        assert!(loaded.terminal_at_ms.is_some());
    }

    #[tokio::test]
    async fn resource_list_by_session_binding_and_node() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_record(&tenant);
        store.insert(&record).await.unwrap();

        let by_session = store
            .list_by_session(&tenant, record.media_session_id.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(by_session.len(), 1);

        let by_binding = store
            .list_by_binding(&tenant, record.media_binding_id.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(by_binding.len(), 1);

        let by_node = store
            .list_by_node(&tenant, record.media_node_id.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(by_node.len(), 1);
    }

    #[tokio::test]
    async fn resource_tombstone_requires_terminal_state() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_record(&tenant);
        store.insert(&record).await.unwrap();

        let err = store
            .tombstone(&tenant, "publisher", "pub-1", ResourceState::Active)
            .await
            .unwrap_err();
        assert!(matches!(err, ControlPlaneError::InvalidArgument(_)));
    }
}
