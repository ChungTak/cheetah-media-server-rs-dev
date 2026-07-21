//! Reconciler and orphan protection for the control plane.
//!
//! REC-01 defines the reconciler inputs; REC-02 implements orphan marking,
//! grace-period confirmation and scoped cleanup.
//!
//! 控制面对账与 orphan 保护。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::admin::{
    AdminIdentity, AdminScope, CleanupOrphanRequest, CleanupOrphanResponse,
};
use cheetah_media_api::error::{MediaError, MediaErrorCode};
use cheetah_media_api::fencing::ControlledResourceRef;
use cheetah_media_api::resource_filter::ResourceState;

use crate::error::ControlPlaneError;
use crate::store::{OrphanStore, ResourceStore};

/// Limits for a single reconciliation pass.
///
/// 单次对账上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileLimits {
    /// Maximum orphan candidates to mark in one pass.
    pub max_mark: u32,
    /// Maximum orphan marks to confirm in one pass.
    pub max_confirm: u32,
    /// Grace period in milliseconds before an unconfirmed orphan becomes
    /// eligible for cleanup.
    pub grace_period_ms: i64,
}

impl Default for ReconcileLimits {
    fn default() -> Self {
        Self {
            max_mark: 100,
            max_confirm: 100,
            grace_period_ms: 30_000,
        }
    }
}

/// Result of a reconciliation pass.
///
/// 对账结果统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    pub orphans_marked: u64,
    pub orphans_confirmed: u64,
    pub orphans_cleaned: u64,
    pub skipped: u64,
}

/// Drives reconciliation between the durable resource index and external
/// signaling state.
///
/// 驱动持久资源索引与外部 signaling 状态对账。
#[async_trait]
pub trait Reconciler: Send + Sync {
    /// Run a bounded reconciliation pass.
    async fn reconcile(
        &self,
        now_ms: i64,
        limits: &ReconcileLimits,
    ) -> Result<ReconcileReport, ControlPlaneError>;

    /// Clean up a typed orphan resource after an admin identity with the
    /// `Orphan` scope has authorized it.
    async fn cleanup_orphan(
        &self,
        identity: &AdminIdentity,
        request: &CleanupOrphanRequest,
    ) -> Result<CleanupOrphanResponse, ControlPlaneError>;
}

/// Reconciler implementation focused on orphan protection.
///
/// Orphan 保护实现。
pub struct OrphanReconciler {
    resources: Arc<dyn ResourceStore>,
    orphan_store: Arc<dyn OrphanStore>,
}

impl OrphanReconciler {
    pub fn new(resources: Arc<dyn ResourceStore>, orphan_store: Arc<dyn OrphanStore>) -> Self {
        Self {
            resources,
            orphan_store,
        }
    }

    /// Mark a single resource as an orphan candidate if it has no binding and
    /// is not already terminal.
    pub async fn mark_orphan(
        &self,
        resource_ref: &ControlledResourceRef,
        now_ms: i64,
    ) -> Result<(), ControlPlaneError> {
        if resource_ref.media_binding_id.is_some() {
            return Err(ControlPlaneError::InvalidArgument(
                "resource still has a binding; cannot mark as orphan".to_string(),
            ));
        }

        let record = self
            .resources
            .get(
                &resource_ref.tenant_id,
                &resource_ref.resource_kind,
                &resource_ref.resource_handle,
            )
            .await?;

        let record = match record {
            Some(r) => r,
            None => {
                return Err(ControlPlaneError::NotFound(
                    "cannot mark missing resource as orphan".to_string(),
                ));
            }
        };

        if record.state.is_terminal() {
            return Ok(());
        }

        if record.media_binding_id.is_some() {
            return Err(ControlPlaneError::InvalidArgument(
                "resource regained a binding; cannot mark as orphan".to_string(),
            ));
        }

        self.orphan_store.mark_orphan(resource_ref, now_ms).await?;
        Ok(())
    }

    /// Confirm orphan marks whose grace period has elapsed and whose resources
    /// are still without a binding.
    pub async fn confirm_orphans(
        &self,
        now_ms: i64,
        limits: &ReconcileLimits,
    ) -> Result<u64, ControlPlaneError> {
        let before_ms = now_ms.saturating_sub(limits.grace_period_ms);
        let candidates = self
            .orphan_store
            .list_unconfirmed_older_than(before_ms, limits.max_confirm)
            .await?;

        let mut confirmed = 0u64;
        for orphan in candidates {
            let resource = self
                .resources
                .get(
                    &orphan.tenant_id,
                    &orphan.resource_kind,
                    &orphan.resource_handle,
                )
                .await?;

            match resource {
                Some(r) if r.media_binding_id.is_none() && !r.state.is_terminal() => {
                    self.orphan_store
                        .confirm_orphan(
                            &orphan.tenant_id,
                            &orphan.resource_kind,
                            &orphan.resource_handle,
                            now_ms,
                        )
                        .await?;
                    confirmed += 1;
                }
                _ => {
                    // The resource has recovered, been bound, or terminated;
                    // remove the stale orphan mark.
                    self.orphan_store
                        .remove_orphan(
                            &orphan.tenant_id,
                            &orphan.resource_kind,
                            &orphan.resource_handle,
                        )
                        .await?;
                }
            }
        }

        Ok(confirmed)
    }

    /// Mark all non-terminal resources without a binding as orphan candidates,
    /// up to `max_mark`.
    async fn mark_candidates(
        &self,
        now_ms: i64,
        limits: &ReconcileLimits,
    ) -> Result<u64, ControlPlaneError> {
        let resources = self.resources.list_non_terminal(limits.max_mark).await?;
        let mut marked = 0u64;
        for record in resources {
            if record.media_binding_id.is_some() || record.state.is_terminal() {
                continue;
            }

            if self
                .orphan_store
                .get_orphan(
                    &record.tenant_id,
                    &record.resource_kind,
                    &record.resource_handle,
                )
                .await?
                .is_some()
            {
                continue;
            }

            let resource_ref = record.resource_ref();
            if self.mark_orphan(&resource_ref, now_ms).await.is_ok() {
                marked += 1;
            }
        }
        Ok(marked)
    }
}

#[async_trait]
impl Reconciler for OrphanReconciler {
    async fn reconcile(
        &self,
        now_ms: i64,
        limits: &ReconcileLimits,
    ) -> Result<ReconcileReport, ControlPlaneError> {
        let orphans_marked = self.mark_candidates(now_ms, limits).await?;
        let orphans_confirmed = self.confirm_orphans(now_ms, limits).await?;
        Ok(ReconcileReport {
            orphans_marked,
            orphans_confirmed,
            ..Default::default()
        })
    }

    async fn cleanup_orphan(
        &self,
        identity: &AdminIdentity,
        request: &CleanupOrphanRequest,
    ) -> Result<CleanupOrphanResponse, ControlPlaneError> {
        if !identity.has_scope(AdminScope::Orphan) {
            return Err(ControlPlaneError::Media(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "admin identity lacks Orphan scope",
            )));
        }

        let resource = self
            .resources
            .get(
                &request.tenant_id,
                &request.resource_kind,
                &request.resource_handle,
            )
            .await?;

        let orphan = self
            .orphan_store
            .get_orphan(
                &request.tenant_id,
                &request.resource_kind,
                &request.resource_handle,
            )
            .await?;

        // Idempotent cleanup: the resource is already gone and the mark is gone.
        if resource.is_none() && orphan.is_none() {
            return Ok(CleanupOrphanResponse { cleaned: true });
        }

        // A resource that has regained a binding must not be cleaned up, even
        // if it was previously marked as a confirmed orphan.
        let eligible = match (&resource, &orphan) {
            (Some(r), _) => r.media_binding_id.is_none(),
            (None, Some(o)) => o.confirmed,
            _ => false,
        };

        if !eligible {
            return Err(ControlPlaneError::InvalidArgument(
                "resource is not an orphan or has not been confirmed".to_string(),
            ));
        }

        if let Some(record) = resource {
            if !record.state.is_terminal() {
                self.resources
                    .tombstone(
                        &request.tenant_id,
                        &request.resource_kind,
                        &request.resource_handle,
                        ResourceState::Failed,
                    )
                    .await?;
            }
        }

        if orphan.is_some() {
            self.orphan_store
                .remove_orphan(
                    &request.tenant_id,
                    &request.resource_kind,
                    &request.resource_handle,
                )
                .await?;
        }

        Ok(CleanupOrphanResponse { cleaned: true })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cheetah_media_api::admin::{AdminIdentity, AdminScope, CleanupOrphanRequest};
    use cheetah_media_api::ids::{
        MediaBindingId, MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId, MediaSessionId,
        OwnerEpoch, ResourceGeneration, TenantId,
    };
    use cheetah_media_api::resource_filter::ResourceState;
    use cheetah_runtime_tokio::TokioRuntime;

    use super::*;
    use crate::sqlite::SqliteStore;
    use crate::store::{now_ms, ResourceRecord, ResourceStore};

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
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            terminal_at_ms: None,
        }
    }

    async fn reconciler() -> (SqliteStore, OrphanReconciler) {
        let store = sqlite().await;
        let resources: Arc<dyn ResourceStore> = Arc::new(store.clone());
        let orphan_store: Arc<dyn OrphanStore> = Arc::new(store.clone());
        (store, OrphanReconciler::new(resources, orphan_store))
    }

    #[tokio::test]
    async fn reconcile_marks_and_confirms_orphan() {
        let (store, reconciler) = reconciler().await;
        let tenant = TenantId::new("tenant-1").unwrap();
        let record = sample_resource(&tenant);
        store.insert(&record).await.unwrap();

        // First pass: mark the orphan candidate.
        let limits = ReconcileLimits {
            max_mark: 10,
            max_confirm: 10,
            grace_period_ms: 1_000,
        };
        let report = reconciler.reconcile(0, &limits).await.unwrap();
        assert_eq!(report.orphans_marked, 1);
        assert_eq!(report.orphans_confirmed, 0);

        // Second pass after grace: confirm the orphan.
        let report = reconciler.reconcile(2_000, &limits).await.unwrap();
        assert_eq!(report.orphans_marked, 0);
        assert_eq!(report.orphans_confirmed, 1);

        // Admin cleanup succeeds with Orphan scope.
        let identity = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Orphan],
        };
        let request = CleanupOrphanRequest {
            tenant_id: tenant.clone(),
            resource_kind: "publisher".to_string(),
            resource_handle: "pub-1".to_string(),
            reason: "grace expired".to_string(),
        };
        let response = reconciler
            .cleanup_orphan(&identity, &request)
            .await
            .unwrap();
        assert!(response.cleaned);

        let record = store
            .get(&tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .unwrap();
        assert!(record.state.is_terminal());
    }

    #[tokio::test]
    async fn cleanup_rejects_missing_scope() {
        let (_, reconciler) = reconciler().await;
        let tenant = TenantId::new("tenant-1").unwrap();
        let identity = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Node],
        };
        let request = CleanupOrphanRequest {
            tenant_id: tenant.clone(),
            resource_kind: "publisher".to_string(),
            resource_handle: "pub-1".to_string(),
            reason: "".to_string(),
        };
        let err = reconciler
            .cleanup_orphan(&identity, &request)
            .await
            .unwrap_err();
        assert!(matches!(err, ControlPlaneError::Media(_)));
    }

    #[tokio::test]
    async fn reconcile_skips_resources_with_binding() {
        let (store, reconciler) = reconciler().await;
        let tenant = TenantId::new("tenant-1").unwrap();
        let mut record = sample_resource(&tenant);
        record.media_binding_id =
            Some(MediaBindingId::new("550e8400-e29b-41d4-a716-446655440002").unwrap());
        store.insert(&record).await.unwrap();

        let limits = ReconcileLimits {
            max_mark: 10,
            max_confirm: 10,
            grace_period_ms: 1_000,
        };
        let report = reconciler.reconcile(0, &limits).await.unwrap();
        assert_eq!(report.orphans_marked, 0);
    }
}
