//! Control-plane `AdminApi` implementation.
//!
//! 控制面 AdminApi 实现。当前版本实现 reconciliation 与 orphan cleanup；
//! 其余管理操作返回 `Unsupported` 直到对应子系统落地。

use async_trait::async_trait;
use cheetah_media_api::admin::{
    AdminApi, AdminIdentity, AdminScope, CheckpointStoreRequest, CheckpointStoreResponse,
    CleanupOrphanRequest, CleanupOrphanResponse, DiagnosticsRequest, DiagnosticsResponse,
    DrainNodeRequest, DrainNodeResponse, ReconcileScope, RotateTlsRequest, RotateTlsResponse,
    TriggerReconciliationRequest, TriggerReconciliationResponse,
};
use cheetah_media_api::error::MediaError;

use crate::facade::ControlPlane;
use crate::reconciler::ReconcileLimits;
use crate::store::now_ms;

#[async_trait]
impl AdminApi for ControlPlane {
    async fn drain_node(
        &self,
        _identity: &AdminIdentity,
        _request: DrainNodeRequest,
    ) -> Result<DrainNodeResponse, MediaError> {
        Err(MediaError::unsupported("drain_node not implemented"))
    }

    async fn trigger_reconciliation(
        &self,
        identity: &AdminIdentity,
        request: TriggerReconciliationRequest,
    ) -> Result<TriggerReconciliationResponse, MediaError> {
        if !identity.has_scope(AdminScope::Reconcile) {
            return Err(MediaError::new(
                cheetah_media_api::error::MediaErrorCode::PermissionDenied,
                "admin identity lacks Reconcile scope",
            ));
        }
        if request.scope != ReconcileScope::All {
            return Err(MediaError::unsupported(
                "scoped reconciliation is not yet implemented",
            ));
        }
        if request.node_id.is_some() || request.tenant_id.is_some() {
            return Err(MediaError::unsupported(
                "node/tenant-scoped reconciliation is not yet implemented",
            ));
        }
        let _report = self
            .reconciler
            .reconcile(now_ms(), &ReconcileLimits::default())
            .await?;
        Ok(TriggerReconciliationResponse { triggered: true })
    }

    async fn inspect_diagnostics(
        &self,
        _identity: &AdminIdentity,
        _request: DiagnosticsRequest,
    ) -> Result<DiagnosticsResponse, MediaError> {
        Err(MediaError::unsupported(
            "inspect_diagnostics not implemented",
        ))
    }

    async fn rotate_tls(
        &self,
        _identity: &AdminIdentity,
        _request: RotateTlsRequest,
    ) -> Result<RotateTlsResponse, MediaError> {
        Err(MediaError::unsupported("rotate_tls not implemented"))
    }

    async fn checkpoint_store(
        &self,
        _identity: &AdminIdentity,
        _request: CheckpointStoreRequest,
    ) -> Result<CheckpointStoreResponse, MediaError> {
        Err(MediaError::unsupported("checkpoint_store not implemented"))
    }

    async fn cleanup_orphan(
        &self,
        identity: &AdminIdentity,
        request: CleanupOrphanRequest,
    ) -> Result<CleanupOrphanResponse, MediaError> {
        self.reconciler.cleanup_orphan(identity, &request).await?;
        Ok(CleanupOrphanResponse { cleaned: true })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cheetah_media_api::admin::{
        AdminApi, AdminIdentity, AdminScope, ReconcileScope, TriggerReconciliationRequest,
    };
    use cheetah_media_api::error::MediaErrorCode;
    use cheetah_runtime_tokio::TokioRuntime;

    use crate::facade::ControlPlane;
    use crate::sqlite::SqliteStore;

    fn runtime() -> Arc<TokioRuntime> {
        Arc::new(TokioRuntime::new())
    }

    async fn control_plane() -> ControlPlane {
        let runtime = runtime();
        let store = SqliteStore::new(runtime.clone(), ":memory:").await.unwrap();
        ControlPlane::new(
            runtime,
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
    }

    #[tokio::test]
    async fn trigger_reconciliation_requires_reconcile_scope() {
        let cp = control_plane().await;
        let req = TriggerReconciliationRequest {
            scope: ReconcileScope::All,
            node_id: None,
            tenant_id: None,
        };

        let no_scope = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Orphan],
        };
        let err = cp
            .trigger_reconciliation(&no_scope, req.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code, MediaErrorCode::PermissionDenied);

        let with_scope = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Reconcile],
        };
        let resp = cp.trigger_reconciliation(&with_scope, req).await.unwrap();
        assert!(resp.triggered);
    }

    #[tokio::test]
    async fn trigger_reconciliation_rejects_scoped_requests() {
        let cp = control_plane().await;
        let identity = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Reconcile],
        };

        let scoped = TriggerReconciliationRequest {
            scope: ReconcileScope::Tenant,
            node_id: None,
            tenant_id: None,
        };
        let err = cp
            .trigger_reconciliation(&identity, scoped)
            .await
            .unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Unsupported);

        let scoped = TriggerReconciliationRequest {
            scope: ReconcileScope::All,
            node_id: Some(
                cheetah_media_api::ids::MediaNodeId::new("550e8400-e29b-41d4-a716-446655440003")
                    .unwrap(),
            ),
            tenant_id: None,
        };
        let err = cp
            .trigger_reconciliation(&identity, scoped)
            .await
            .unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Unsupported);
    }
}
