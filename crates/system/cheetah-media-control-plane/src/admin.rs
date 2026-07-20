//! Control-plane `AdminApi` implementation.
//!
//! 控制面 AdminApi 实现。当前版本实现 reconciliation 与 orphan cleanup；
//! 其余管理操作返回 `Unsupported` 直到对应子系统落地。

use async_trait::async_trait;
use cheetah_media_api::admin::{
    AdminApi, AdminIdentity, CheckpointStoreRequest, CheckpointStoreResponse, CleanupOrphanRequest,
    CleanupOrphanResponse, DiagnosticsRequest, DiagnosticsResponse, DrainNodeRequest,
    DrainNodeResponse, RotateTlsRequest, RotateTlsResponse, TriggerReconciliationRequest,
    TriggerReconciliationResponse,
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
        _identity: &AdminIdentity,
        _request: TriggerReconciliationRequest,
    ) -> Result<TriggerReconciliationResponse, MediaError> {
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
