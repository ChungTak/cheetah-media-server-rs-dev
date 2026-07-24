//! Control-plane store traits.
//!
//! 控制面 store trait。

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cheetah_media_api::error::{EffectOutcome, MediaError};
use cheetah_media_api::fencing::{ControlledResourceRef, ResourceOrigin};
use cheetah_media_api::ids::{
    MediaBindingId, MediaKey, MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId,
    MediaSessionId, OwnerEpoch, ResourceGeneration, TenantId,
};
use cheetah_media_api::resource_filter::ResourceState;
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

    /// List idempotency records in `Prepared` or `Unknown` state, limited to
    /// `max_records`.
    ///
    /// Used by startup recovery to find in-flight or ambiguous attempts.
    async fn list_prepared_unknown(
        &self,
        max_records: u32,
    ) -> Result<Vec<IdempotencyRecord>, ControlPlaneError>;
}

/// A durable controlled-resource record.
///
/// 持久化受控资源记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRecord {
    pub tenant_id: TenantId,
    pub resource_kind: String,
    pub resource_handle: String,
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    pub media_key: Option<MediaKey>,
    pub idempotency_scope: Option<String>,
    pub canonical_digest: Option<CanonicalDigest>,
    pub accepted_owner_epoch: OwnerEpoch,
    pub media_node_id: Option<MediaNodeId>,
    pub media_node_instance_id: Option<MediaNodeInstanceId>,
    pub media_node_instance_epoch: MediaNodeInstanceEpoch,
    pub generation: ResourceGeneration,
    pub state: ResourceState,
    pub safe_last_error: Option<MediaError>,
    /// Whether the resource was created by cluster signaling or a local adapter.
    pub origin: ResourceOrigin,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal_at_ms: Option<i64>,
}

impl ResourceRecord {
    /// Return the stable `ControlledResourceRef` represented by this record.
    pub fn resource_ref(&self) -> ControlledResourceRef {
        ControlledResourceRef {
            tenant_id: self.tenant_id.clone(),
            media_session_id: self.media_session_id.clone(),
            media_binding_id: self.media_binding_id.clone(),
            resource_kind: self.resource_kind.clone(),
            resource_handle: self.resource_handle.clone(),
            owner_epoch: self.accepted_owner_epoch,
            node_instance_epoch: self.media_node_instance_epoch,
            generation: self.generation,
            origin: self.origin,
        }
    }
}

/// Aggregate counts returned by store diagnostics.
///
/// store 诊断聚合计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoreStats {
    pub resource_count: u64,
    pub non_terminal_resource_count: u64,
    pub event_count: u64,
}

/// Cold-path store maintenance operations (checkpoint/compact/stats).
///
/// 冷路径 store 维护操作。
#[async_trait]
pub trait StoreMaintenance: Send + Sync {
    /// Checkpoint or compact the durable store.
    async fn checkpoint(
        &self,
        kind: cheetah_media_api::admin::CheckpointKind,
    ) -> Result<(), ControlPlaneError>;

    /// Return safe aggregate counts, optionally filtered by tenant/kind.
    async fn stats(
        &self,
        tenant_id: Option<&TenantId>,
        resource_kind: Option<&str>,
    ) -> Result<StoreStats, ControlPlaneError>;
}

/// Durable controlled-resource storage.
///
/// 持久化受控资源存储。
#[async_trait]
pub trait ResourceStore: Send + Sync {
    /// Return the resource with the given tenant and resource kind/handle.
    async fn get(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
    ) -> Result<Option<ResourceRecord>, ControlPlaneError>;

    /// Insert a new resource record.
    ///
    /// Fails with `Conflict` if a record with the same `(tenant, kind, handle)`
    /// already exists.
    async fn insert(&self, record: &ResourceRecord) -> Result<(), ControlPlaneError>;

    /// Atomically advance the accepted owner epoch if it equals `expected`.
    ///
    /// Returns `true` if the update succeeded.
    async fn compare_and_set_owner_epoch(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        expected: OwnerEpoch,
        new: OwnerEpoch,
    ) -> Result<bool, ControlPlaneError>;

    /// Atomically advance the generation and update the state if the current
    /// generation equals `expected`.
    ///
    /// Returns `true` if the update succeeded.
    async fn compare_and_set_generation(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        expected: ResourceGeneration,
        new: ResourceGeneration,
        state: ResourceState,
    ) -> Result<bool, ControlPlaneError>;

    /// Set the resource state. Terminal states record `terminal_at_ms`.
    async fn set_state(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        state: ResourceState,
    ) -> Result<(), ControlPlaneError>;

    /// Mark the resource as terminal with the given state and timestamp.
    ///
    /// `state` must be terminal.
    async fn tombstone(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        state: ResourceState,
    ) -> Result<(), ControlPlaneError>;

    /// List resources for a session.
    async fn list_by_session(
        &self,
        tenant_id: &TenantId,
        session_id: &MediaSessionId,
    ) -> Result<Vec<ResourceRecord>, ControlPlaneError>;

    /// List resources for a binding.
    async fn list_by_binding(
        &self,
        tenant_id: &TenantId,
        binding_id: &MediaBindingId,
    ) -> Result<Vec<ResourceRecord>, ControlPlaneError>;

    /// List resources assigned to a node.
    async fn list_by_node(
        &self,
        tenant_id: &TenantId,
        node_id: &MediaNodeId,
    ) -> Result<Vec<ResourceRecord>, ControlPlaneError>;

    /// List all resources in a non-terminal state, limited to `max_records`.
    ///
    /// Used by startup recovery to find resources that may need reconciliation.
    async fn list_non_terminal(
        &self,
        max_records: u32,
    ) -> Result<Vec<ResourceRecord>, ControlPlaneError>;
}

/// A durable orphan mark for a controlled resource that has lost its signaling
/// binding and is waiting through a grace period before it may be cleaned up.
///
/// 持久化 orphan 标记，用于丢失 signaling binding 的受控资源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanRecord {
    pub tenant_id: TenantId,
    pub resource_kind: String,
    pub resource_handle: String,
    pub resource_ref_json: String,
    pub marked_at_ms: i64,
    pub confirmed: bool,
    pub confirmed_at_ms: Option<i64>,
}

/// Durable orphan tracking.
///
/// Implementations must not expose `rusqlite` or other database connection types
/// through the trait.
#[async_trait]
pub trait OrphanStore: Send + Sync {
    /// Mark a resource as an orphan candidate. Fails with `Conflict` if the
    /// resource is already marked and confirmed.
    async fn mark_orphan(
        &self,
        resource_ref: &ControlledResourceRef,
        now_ms: i64,
    ) -> Result<(), ControlPlaneError>;

    /// Fetch the orphan mark for a resource, if any.
    async fn get_orphan(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
    ) -> Result<Option<OrphanRecord>, ControlPlaneError>;

    /// Confirm an orphan mark. Confirmed orphans are eligible for cleanup after
    /// admin authorization.
    async fn confirm_orphan(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
        now_ms: i64,
    ) -> Result<(), ControlPlaneError>;

    /// List unconfirmed orphan marks up to `max_records`.
    async fn list_unconfirmed(
        &self,
        max_records: u32,
    ) -> Result<Vec<OrphanRecord>, ControlPlaneError>;

    /// List unconfirmed orphan marks whose `marked_at_ms` is at or before
    /// `before_ms`, limited to `max_records`.
    async fn list_unconfirmed_older_than(
        &self,
        before_ms: i64,
        max_records: u32,
    ) -> Result<Vec<OrphanRecord>, ControlPlaneError>;

    /// Remove the orphan mark for a resource.
    async fn remove_orphan(
        &self,
        tenant_id: &TenantId,
        resource_kind: &str,
        resource_handle: &str,
    ) -> Result<(), ControlPlaneError>;
}

/// Return the current time in milliseconds since the Unix epoch.
#[allow(dead_code)]
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_millis() as i64
}
