//! Control-plane store traits.
//!
//! 控制面 store trait。

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_media_api::error::EffectOutcome;
use cheetah_media_api::fencing::ControlledResourceRef;
use serde_json::Value;

use crate::error::ControlPlaneError;
use crate::idempotency::{CanonicalDigest, IdempotencyKey, IdempotencyState};

/// Outcome returned when an idempotency key is prepared or looked up.
///
/// 准备或查询幂等键时返回的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyOutcome {
    /// The key is new or prepared; the caller should execute the side effect.
    Proceed,
    /// The key has already completed with the same canonical digest; replay
    /// the stored result.
    Replay(Box<IdempotencyRecord>),
    /// The key exists with a different digest; the request conflicts.
    Conflict,
    /// The key is in an ambiguous state and must be reconciled before retry.
    Reconcile,
}

/// A durable idempotency record.
///
/// 持久化幂等记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecord {
    pub key: IdempotencyKey,
    pub state: IdempotencyState,
    pub canonical_digest: CanonicalDigest,
    pub resource_ref: Option<ControlledResourceRef>,
    pub effect_outcome: EffectOutcome,
    /// Serialized domain result suitable for replay to the caller.
    pub serialized_domain_result: Option<Value>,
    /// Safe error recorded when the operation failed.
    pub safe_error: Option<cheetah_media_api::error::MediaError>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub expires_at_ms: i64,
    pub attempt_count: u32,
}

/// Durable idempotency storage.
///
/// Implementations must guarantee unique `(tenant_id, operation_kind,
/// idempotency_key)` records and must not expose `rusqlite` or other database
/// connection types through the trait.
///
/// 持久化幂等存储。
#[async_trait]
pub trait IdempotencyStore: Send + Sync {
    /// Return the existing record for a key, if any.
    async fn get(
        &self,
        key: &IdempotencyKey,
    ) -> Result<Option<IdempotencyRecord>, ControlPlaneError>;

    /// Atomically insert or validate a `Prepared` record for the key.
    ///
    /// - If the key does not exist, a `Prepared` record is inserted and
    ///   `Proceed` is returned.
    /// - If the key exists with the same digest and state is `Completed` or
    ///   `Failed`, `Replay` is returned.
    /// - If the key exists with a different digest, `Conflict` is returned.
    /// - If the key exists in `Unknown` or `Prepared`, `Reconcile` is returned.
    async fn prepare(
        &self,
        key: &IdempotencyKey,
        digest: CanonicalDigest,
        expires_at_ms: i64,
    ) -> Result<IdempotencyOutcome, ControlPlaneError>;

    /// Persist the final outcome of a side effect.
    ///
    /// The store may overwrite a `Prepared` record but must not silently
    /// overwrite a completed record with a different digest.
    async fn complete(&self, record: &IdempotencyRecord) -> Result<(), ControlPlaneError>;
}

/// Return the current time in milliseconds since the Unix epoch.
#[allow(dead_code)]
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before Unix epoch")
        .as_millis() as i64
}
