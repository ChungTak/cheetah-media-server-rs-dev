//! SQLite-backed control-plane store.
//!
//! SQLite I/O is confined to `RuntimeApi::spawn_blocking`. Each public async
//! method opens a connection in a blocking worker, runs the query, and returns
//! the result. Schema migrations are applied on construction.
//!
//! SQLite 持久化控制面 store。所有 I/O 都通过 `RuntimeApi::spawn_blocking` 在阻塞线程中执行。

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cheetah_media_api::error::EffectOutcome;
use cheetah_runtime_api::RuntimeApi;
use rusqlite::{params, Connection, OptionalExtension};

use crate::blocking::blocking_call;
use crate::error::ControlPlaneError;
use crate::idempotency::{CanonicalDigest, IdempotencyKey, IdempotencyState};
use crate::store::{now_ms, IdempotencyOutcome, IdempotencyRecord, IdempotencyStore};

impl From<rusqlite::Error> for ControlPlaneError {
    fn from(e: rusqlite::Error) -> Self {
        ControlPlaneError::Db(e.to_string())
    }
}

/// A SQLite-backed control-plane store.
///
/// The internal `Connection` is held behind a `std::sync::Mutex` and only
/// accessed from `RuntimeApi::spawn_blocking` workers so it never blocks an
/// async runtime thread.
///
/// 基于 SQLite 的控制面 store。内部 `Connection` 由 `std::sync::Mutex` 保护，
/// 只在 `RuntimeApi::spawn_blocking` worker 中访问。
#[derive(Clone)]
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    runtime: Arc<dyn RuntimeApi>,
}

impl SqliteStore {
    /// Open (or create) the SQLite store at `path` and run migrations.
    pub async fn new(
        runtime: Arc<dyn RuntimeApi>,
        path: impl AsRef<Path>,
    ) -> Result<Self, ControlPlaneError> {
        let path = path.as_ref().to_string_lossy().into_owned();
        let runtime_clone = runtime.clone();
        let conn = blocking_call(
            runtime.as_ref(),
            "sqlite_open",
            move || -> Result<Connection, ControlPlaneError> {
                let mut conn = open_conn(&path)?;
                migrate(&mut conn)?;
                Ok(conn)
            },
        )
        .await??;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            runtime: runtime_clone,
        })
    }

    async fn with_conn<R, F>(&self, name: &str, f: F) -> Result<R, ControlPlaneError>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<R, ControlPlaneError> + Send + 'static,
    {
        let conn = self.conn.clone();
        let runtime = self.runtime.clone();
        let result: Result<R, ControlPlaneError> = blocking_call(
            runtime.as_ref(),
            name,
            move || -> Result<R, ControlPlaneError> {
                let mut guard = conn.lock().map_err(|_| {
                    ControlPlaneError::RuntimeError("sqlite mutex poisoned".to_string())
                })?;
                f(&mut guard)
            },
        )
        .await?;
        result
    }
}

fn open_conn(path: &str) -> Result<Connection, ControlPlaneError> {
    Connection::open(path).map_err(|e| ControlPlaneError::StoreUnavailable(e.to_string()))
}

fn migrate(conn: &mut Connection) -> Result<(), ControlPlaneError> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;

         CREATE TABLE IF NOT EXISTS control_meta (
             schema_version INTEGER PRIMARY KEY,
             stable_node_id TEXT,
             process_instance_id TEXT,
             last_accepted_instance_epoch INTEGER,
             last_contract_version TEXT,
             last_descriptor_checksum TEXT,
             event_sequence INTEGER DEFAULT 0
         );

         CREATE TABLE IF NOT EXISTS idempotency_records (
             tenant_id TEXT NOT NULL,
             operation_kind TEXT NOT NULL,
             idempotency_key TEXT NOT NULL,
             canonical_digest TEXT NOT NULL,
             state TEXT NOT NULL,
             resource_ref TEXT,
             effect_outcome TEXT NOT NULL,
             serialized_domain_result TEXT,
             safe_error TEXT,
             created_at_ms INTEGER NOT NULL,
             updated_at_ms INTEGER NOT NULL,
             expires_at_ms INTEGER NOT NULL,
             attempt_count INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (tenant_id, operation_kind, idempotency_key)
         ) WITHOUT ROWID;

         CREATE INDEX IF NOT EXISTS idx_idempotency_expires
             ON idempotency_records(expires_at_ms);

         CREATE TABLE IF NOT EXISTS controlled_resources (
             tenant_id TEXT NOT NULL,
             resource_kind TEXT NOT NULL,
             resource_handle TEXT NOT NULL,
             media_session_id TEXT,
             media_binding_id TEXT,
             media_key TEXT,
             idempotency_scope TEXT,
             canonical_digest TEXT,
             accepted_owner_epoch INTEGER,
             media_node_id TEXT,
             media_node_instance_id TEXT,
             media_node_instance_epoch INTEGER,
             generation INTEGER NOT NULL,
             state TEXT NOT NULL,
             safe_last_error TEXT,
             created_at_ms INTEGER NOT NULL,
             updated_at_ms INTEGER NOT NULL,
             terminal_at_ms INTEGER,
             PRIMARY KEY (tenant_id, resource_kind, resource_handle)
         ) WITHOUT ROWID;

         CREATE INDEX IF NOT EXISTS idx_resource_session
             ON controlled_resources(tenant_id, media_session_id);
         CREATE INDEX IF NOT EXISTS idx_resource_binding
             ON controlled_resources(tenant_id, media_binding_id);
         CREATE INDEX IF NOT EXISTS idx_resource_idempotency
             ON controlled_resources(tenant_id, idempotency_scope);

         CREATE TABLE IF NOT EXISTS media_events (
             instance_epoch INTEGER NOT NULL,
             sequence INTEGER NOT NULL,
             event_id TEXT NOT NULL,
             tenant_id TEXT NOT NULL,
             resource_kind TEXT,
             resource_handle TEXT,
             occurred_at INTEGER NOT NULL,
             event_kind TEXT NOT NULL,
             serialized_payload TEXT NOT NULL,
             correlation_id TEXT,
             traceparent TEXT,
             tracestate TEXT,
             expires_at INTEGER NOT NULL,
             PRIMARY KEY (instance_epoch, sequence)
         ) WITHOUT ROWID;

         CREATE INDEX IF NOT EXISTS idx_media_events_tenant
             ON media_events(tenant_id, instance_epoch, sequence);
         CREATE INDEX IF NOT EXISTS idx_media_events_id
             ON media_events(event_id);",
    )
    .map_err(|e| ControlPlaneError::StoreUnavailable(e.to_string()))?;

    conn.execute(
        "INSERT OR IGNORE INTO control_meta (schema_version) VALUES (?1)",
        params![1],
    )
    .map_err(|e| ControlPlaneError::StoreUnavailable(e.to_string()))?;
    Ok(())
}

fn select_idempotency_row(
    tx: &rusqlite::Transaction<'_>,
    key: &IdempotencyKey,
) -> Result<Option<RowIdempotency>, rusqlite::Error> {
    let mut stmt = tx.prepare(
        "SELECT canonical_digest, state, resource_ref,
                effect_outcome, serialized_domain_result, safe_error,
                created_at_ms, updated_at_ms, expires_at_ms, attempt_count
         FROM idempotency_records
         WHERE tenant_id = ?1 AND operation_kind = ?2 AND idempotency_key = ?3",
    )?;
    stmt.query_row(
        params![key.tenant_id.as_str(), key.operation_kind, key.key,],
        |row| {
            Ok(RowIdempotency {
                digest: row.get::<_, String>(0)?,
                state: row.get::<_, String>(1)?,
                resource_ref: row.get::<_, Option<String>>(2)?,
                effect_outcome: row.get::<_, String>(3)?,
                serialized_domain_result: row.get::<_, Option<String>>(4)?,
                safe_error: row.get::<_, Option<String>>(5)?,
                created_at_ms: row.get::<_, i64>(6)?,
                updated_at_ms: row.get::<_, i64>(7)?,
                expires_at_ms: row.get::<_, i64>(8)?,
                attempt_count: row.get::<_, u32>(9)?,
            })
        },
    )
    .optional()
}

fn delete_idempotency_row(
    tx: &rusqlite::Transaction<'_>,
    key: &IdempotencyKey,
) -> Result<usize, rusqlite::Error> {
    tx.execute(
        "DELETE FROM idempotency_records
         WHERE tenant_id = ?1 AND operation_kind = ?2 AND idempotency_key = ?3",
        params![key.tenant_id.as_str(), key.operation_kind, key.key,],
    )
}

#[async_trait]
impl IdempotencyStore for SqliteStore {
    async fn get(
        &self,
        key: &IdempotencyKey,
    ) -> Result<Option<IdempotencyRecord>, ControlPlaneError> {
        let key = key.clone();
        self.with_conn("idempotency_get", move |conn| {
            let tx = conn.transaction()?;
            let row = select_idempotency_row(&tx, &key)?;
            if let Some(r) = row {
                if r.expires_at_ms < now_ms() {
                    delete_idempotency_row(&tx, &key)?;
                    tx.commit()?;
                    return Ok(None);
                }
                tx.commit()?;
                return Ok(Some(r.into_record(&key)));
            }
            tx.commit()?;
            Ok(None)
        })
        .await
    }

    async fn prepare(
        &self,
        key: &IdempotencyKey,
        digest: CanonicalDigest,
        expires_at_ms: i64,
    ) -> Result<IdempotencyOutcome, ControlPlaneError> {
        let key = key.clone();
        let digest_hex = digest.to_hex();
        let now = now_ms();
        self.with_conn("idempotency_prepare", move |conn| {
            let tx = conn.transaction()?;

            let existing = select_idempotency_row(&tx, &key)?;

            if let Some(row) = existing {
                // Expired records are deleted so the key can be reused after its
                // TTL. An expired record is not replayed or reconciled.
                if row.expires_at_ms < now {
                    delete_idempotency_row(&tx, &key)?;
                } else {
                    let record = row.into_record(&key);
                    tx.commit()?;
                    if record.canonical_digest != digest {
                        return Ok(IdempotencyOutcome::Conflict);
                    }
                    return match record.state {
                        IdempotencyState::Completed | IdempotencyState::Failed => {
                            Ok(IdempotencyOutcome::Replay(Box::new(record)))
                        }
                        IdempotencyState::Prepared | IdempotencyState::Unknown => {
                            Ok(IdempotencyOutcome::Reconcile)
                        }
                    };
                }
            }

            tx.execute(
                "INSERT INTO idempotency_records
                 (tenant_id, operation_kind, idempotency_key, canonical_digest, state,
                  resource_ref, effect_outcome, serialized_domain_result, safe_error,
                  created_at_ms, updated_at_ms, expires_at_ms, attempt_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL, NULL, ?7, ?7, ?8, 0)",
                params![
                    key.tenant_id.as_str(),
                    key.operation_kind,
                    key.key,
                    digest_hex,
                    state_to_str(IdempotencyState::Prepared),
                    effect_outcome_to_str(EffectOutcome::NotApplied),
                    now,
                    expires_at_ms,
                ],
            )?;
            tx.commit()?;
            Ok(IdempotencyOutcome::Proceed)
        })
        .await
    }

    async fn complete(&self, record: &IdempotencyRecord) -> Result<(), ControlPlaneError> {
        let record = record.clone();
        self.with_conn("idempotency_complete", move |conn| {
            let tx = conn.transaction()?;

            let existing = select_idempotency_row(&tx, &record.key)?;
            let (created_at_ms, attempt_count) = if let Some(row) = existing {
                if parse_hex(&row.digest) != Some(record.canonical_digest) {
                    return Err(ControlPlaneError::InvalidIdempotencyState);
                }
                (row.created_at_ms, row.attempt_count + 1)
            } else {
                (record.created_at_ms, record.attempt_count)
            };

            let resource_ref_json = record
                .resource_ref
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
            let domain_result_json = record
                .serialized_domain_result
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
            let safe_error_json = record
                .safe_error
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;

            tx.execute(
                "INSERT OR REPLACE INTO idempotency_records
                 (tenant_id, operation_kind, idempotency_key, canonical_digest, state,
                  resource_ref, effect_outcome, serialized_domain_result, safe_error,
                  created_at_ms, updated_at_ms, expires_at_ms, attempt_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    record.key.tenant_id.as_str(),
                    record.key.operation_kind,
                    record.key.key,
                    record.canonical_digest.to_hex(),
                    state_to_str(record.state),
                    resource_ref_json,
                    effect_outcome_to_str(record.effect_outcome),
                    domain_result_json,
                    safe_error_json,
                    created_at_ms,
                    record.updated_at_ms,
                    record.expires_at_ms,
                    attempt_count,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}

struct RowIdempotency {
    digest: String,
    state: String,
    resource_ref: Option<String>,
    effect_outcome: String,
    serialized_domain_result: Option<String>,
    safe_error: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    expires_at_ms: i64,
    attempt_count: u32,
}

impl RowIdempotency {
    fn into_record(self, key: &IdempotencyKey) -> IdempotencyRecord {
        IdempotencyRecord {
            key: key.clone(),
            state: str_to_state(&self.state),
            canonical_digest: parse_hex(&self.digest).unwrap_or(CanonicalDigest([0u8; 32])),
            resource_ref: self
                .resource_ref
                .and_then(|s| serde_json::from_str(&s).ok()),
            effect_outcome: str_to_effect_outcome(&self.effect_outcome),
            serialized_domain_result: self
                .serialized_domain_result
                .and_then(|s| serde_json::from_str(&s).ok()),
            safe_error: self.safe_error.and_then(|s| serde_json::from_str(&s).ok()),
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            expires_at_ms: self.expires_at_ms,
            attempt_count: self.attempt_count,
        }
    }
}

fn state_to_str(state: IdempotencyState) -> &'static str {
    match state {
        IdempotencyState::Prepared => "prepared",
        IdempotencyState::Completed => "completed",
        IdempotencyState::Failed => "failed",
        IdempotencyState::Unknown => "unknown",
    }
}

fn str_to_state(s: &str) -> IdempotencyState {
    match s {
        "completed" => IdempotencyState::Completed,
        "failed" => IdempotencyState::Failed,
        "unknown" => IdempotencyState::Unknown,
        _ => IdempotencyState::Prepared,
    }
}

fn effect_outcome_to_str(outcome: EffectOutcome) -> &'static str {
    match outcome {
        EffectOutcome::NotApplied => "not_applied",
        EffectOutcome::Applied => "applied",
        EffectOutcome::Unknown => "unknown",
    }
}

fn str_to_effect_outcome(s: &str) -> EffectOutcome {
    match s {
        "applied" => EffectOutcome::Applied,
        "unknown" => EffectOutcome::Unknown,
        _ => EffectOutcome::NotApplied,
    }
}

fn parse_hex(s: &str) -> Option<CanonicalDigest> {
    if s.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let chunk_str = std::str::from_utf8(chunk).ok()?;
        bytes[i] = u8::from_str_radix(chunk_str, 16).ok()?;
    }
    Some(CanonicalDigest(bytes))
}

#[cfg(test)]
mod tests {
    use cheetah_media_api::error::{MediaError, MediaErrorCode};
    use cheetah_media_api::ids::TenantId;
    use cheetah_runtime_tokio::TokioRuntime;
    use serde_json::Value;

    use super::*;

    #[tokio::test]
    async fn idempotency_prepare_and_complete_round_trip() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let key = IdempotencyKey::new(tenant, "create_session", "key-1");
        let digest = CanonicalDigest([1u8; 32]);

        let outcome = store
            .prepare(&key, digest, now_ms() + 60_000)
            .await
            .unwrap();
        assert!(matches!(outcome, IdempotencyOutcome::Proceed));

        let outcome = store
            .prepare(&key, digest, now_ms() + 60_000)
            .await
            .unwrap();
        assert!(matches!(outcome, IdempotencyOutcome::Reconcile));

        let record = IdempotencyRecord {
            key: key.clone(),
            state: IdempotencyState::Completed,
            canonical_digest: digest,
            resource_ref: None,
            effect_outcome: EffectOutcome::Applied,
            serialized_domain_result: Some(Value::String("ok".to_string())),
            safe_error: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            expires_at_ms: now_ms() + 60_000,
            attempt_count: 1,
        };
        store.complete(&record).await.unwrap();

        let outcome = store
            .prepare(&key, digest, now_ms() + 60_000)
            .await
            .unwrap();
        assert!(matches!(outcome, IdempotencyOutcome::Replay(_)));

        let loaded = store.get(&key).await.unwrap().expect("record exists");
        assert_eq!(loaded.key, key);
        assert_eq!(loaded.state, IdempotencyState::Completed);
    }

    #[tokio::test]
    async fn idempotency_expired_record_allows_reuse() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let key = IdempotencyKey::new(tenant, "create_session", "key-1");
        let digest = CanonicalDigest([1u8; 32]);

        let past = now_ms() - 1;
        store.prepare(&key, digest, past).await.unwrap();
        assert!(store.get(&key).await.unwrap().is_none());

        let outcome = store
            .prepare(&key, digest, now_ms() + 60_000)
            .await
            .unwrap();
        assert!(matches!(outcome, IdempotencyOutcome::Proceed));
    }

    #[tokio::test]
    async fn idempotency_conflict_on_different_digest() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let key = IdempotencyKey::new(tenant, "create_session", "key-1");
        let digest1 = CanonicalDigest([1u8; 32]);
        let digest2 = CanonicalDigest([2u8; 32]);

        store
            .prepare(&key, digest1, now_ms() + 60_000)
            .await
            .unwrap();
        let outcome = store
            .prepare(&key, digest2, now_ms() + 60_000)
            .await
            .unwrap();
        assert!(matches!(outcome, IdempotencyOutcome::Conflict));
    }

    #[tokio::test]
    async fn idempotency_safe_error_round_trips() {
        let rt = Arc::new(TokioRuntime::new());
        let store = SqliteStore::new(rt, ":memory:").await.unwrap();

        let tenant = TenantId::new("tenant-1").unwrap();
        let key = IdempotencyKey::new(tenant, "create_session", "key-1");
        let digest = CanonicalDigest([3u8; 32]);

        store
            .prepare(&key, digest, now_ms() + 60_000)
            .await
            .unwrap();

        let record = IdempotencyRecord {
            key: key.clone(),
            state: IdempotencyState::Failed,
            canonical_digest: digest,
            resource_ref: None,
            effect_outcome: EffectOutcome::NotApplied,
            serialized_domain_result: None,
            safe_error: Some(MediaError::new(MediaErrorCode::Internal, "boom")),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            expires_at_ms: now_ms() + 60_000,
            attempt_count: 1,
        };
        store.complete(&record).await.unwrap();

        let loaded = store.get(&key).await.unwrap().expect("record exists");
        assert_eq!(loaded.safe_error, record.safe_error);
    }
}
