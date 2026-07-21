//! Control-plane `AdminApi` implementation.
//!
//! 控制面 AdminApi 实现：drain、reconciliation、diagnostics、TLS 轮换与 store 维护。

use async_trait::async_trait;
use cheetah_media_api::admin::{
    AdminApi, AdminIdentity, AdminScope, CheckpointStoreRequest, CheckpointStoreResponse,
    CleanupOrphanRequest, CleanupOrphanResponse, DiagnosticsRequest, DiagnosticsResponse,
    DrainNodeRequest, DrainNodeResponse, ReconcileScope, RotateTlsRequest, RotateTlsResponse,
    TriggerReconciliationRequest, TriggerReconciliationResponse,
};
use cheetah_media_api::error::{MediaError, MediaErrorCode};
use cheetah_media_api::fencing::NodeState;
use cheetah_media_api::port::MediaCapacityApi;

use crate::facade::ControlPlane;
use crate::reconciler::ReconcileLimits;
use crate::store::now_ms;

#[async_trait]
impl AdminApi for ControlPlane {
    async fn drain_node(
        &self,
        identity: &AdminIdentity,
        request: DrainNodeRequest,
    ) -> Result<DrainNodeResponse, MediaError> {
        if !identity.has_scope(AdminScope::Node) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "admin identity lacks Node scope",
            ));
        }

        // Prefer the full NODE supervisor when assembled; fall back to the
        // lightweight node snapshot used by unit tests and partial wiring.
        if let Some(sup) = &self.node_supervisor {
            let rt = sup.runtime_state().ok_or_else(|| {
                MediaError::new(
                    MediaErrorCode::Unavailable,
                    "node runtime state is not initialized",
                )
            })?;
            if rt.node_id != request.node_id {
                return Err(MediaError::new(
                    MediaErrorCode::NotFound,
                    "drain target node does not match this process",
                ));
            }
            if request.drain {
                let resp = sup
                    .drain(cheetah_media_api::node::NodeDrainRequest {
                        drain_deadline_ms: now_ms().saturating_add(30_000),
                        reason: request.reason.clone(),
                        force: false,
                    })
                    .await?;
                self.sync_node_from_supervisor();
                return Ok(DrainNodeResponse {
                    node_id: request.node_id,
                    draining: resp.accepted,
                });
            }
            sup.leave_drain().await?;
            self.sync_node_from_supervisor();
            return Ok(DrainNodeResponse {
                node_id: request.node_id,
                draining: false,
            });
        }

        let open_gate = {
            let mut guard = self.node.lock().map_err(|_| {
                MediaError::new(MediaErrorCode::Internal, "node runtime mutex poisoned")
            })?;
            let Some(state) = guard.as_mut() else {
                return Err(MediaError::new(
                    MediaErrorCode::Unavailable,
                    "node runtime state is not initialized",
                ));
            };
            if state.node_id != request.node_id {
                return Err(MediaError::new(
                    MediaErrorCode::NotFound,
                    "drain target node does not match this process",
                ));
            }

            if request.drain {
                state.state = NodeState::Draining;
                false
            } else if state.lease.status == cheetah_media_api::fencing::LeaseStatus::Active {
                // Leaving drain returns Active and re-opens the create gate only
                // when a lease is still active.
                state.state = NodeState::Active;
                true
            } else {
                state.state = NodeState::Isolated;
                false
            }
        };

        if let Some(capacity) = &self.capacity {
            capacity.set_node_gate(open_gate).await?;
        }

        Ok(DrainNodeResponse {
            node_id: request.node_id,
            draining: request.drain,
        })
    }

    async fn trigger_reconciliation(
        &self,
        identity: &AdminIdentity,
        request: TriggerReconciliationRequest,
    ) -> Result<TriggerReconciliationResponse, MediaError> {
        if !identity.has_scope(AdminScope::Reconcile) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "admin identity lacks Reconcile scope",
            ));
        }
        match request.scope {
            ReconcileScope::All => {
                if request.node_id.is_some() || request.tenant_id.is_some() {
                    return Err(MediaError::invalid_argument(
                        "All scope must not set node_id or tenant_id",
                    ));
                }
            }
            ReconcileScope::Node => {
                if request.node_id.is_none() {
                    return Err(MediaError::invalid_argument("Node scope requires node_id"));
                }
            }
            ReconcileScope::Tenant => {
                if request.tenant_id.is_none() {
                    return Err(MediaError::invalid_argument(
                        "Tenant scope requires tenant_id",
                    ));
                }
            }
        }

        let _report = self
            .reconciler
            .reconcile_scoped(
                now_ms(),
                &ReconcileLimits::default(),
                request.scope,
                request.node_id.as_ref(),
                request.tenant_id.as_ref(),
            )
            .await?;
        Ok(TriggerReconciliationResponse { triggered: true })
    }

    async fn inspect_diagnostics(
        &self,
        identity: &AdminIdentity,
        request: DiagnosticsRequest,
    ) -> Result<DiagnosticsResponse, MediaError> {
        if !identity.has_scope(AdminScope::Reconcile) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "admin identity lacks Reconcile scope",
            ));
        }
        let Some(maintenance) = &self.store_maintenance else {
            return Err(MediaError::unavailable(
                "store maintenance is not configured",
            ));
        };
        let stats = maintenance
            .stats(request.tenant_id.as_ref(), request.resource_kind.as_deref())
            .await?;
        let node_count = if self.node_runtime().is_some() { 1 } else { 0 };
        let _ = request.max_events; // reserved for future event-sample summaries
        Ok(DiagnosticsResponse {
            node_count,
            resource_count: stats.resource_count,
            event_count: stats.event_count,
            non_terminal_resource_count: stats.non_terminal_resource_count,
        })
    }

    async fn rotate_tls(
        &self,
        identity: &AdminIdentity,
        request: RotateTlsRequest,
    ) -> Result<RotateTlsResponse, MediaError> {
        if !identity.has_scope(AdminScope::Tls) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "admin identity lacks Tls scope",
            ));
        }
        let Some(rotator) = &self.tls_rotator else {
            return Err(MediaError::unavailable(
                "TLS rotator is not configured for this process",
            ));
        };
        let applied = rotator.rotate(request.component).await?;
        Ok(RotateTlsResponse { applied })
    }

    async fn checkpoint_store(
        &self,
        identity: &AdminIdentity,
        request: CheckpointStoreRequest,
    ) -> Result<CheckpointStoreResponse, MediaError> {
        if !identity.has_scope(AdminScope::Store) {
            return Err(MediaError::new(
                MediaErrorCode::PermissionDenied,
                "admin identity lacks Store scope",
            ));
        }
        let Some(maintenance) = &self.store_maintenance else {
            return Err(MediaError::unavailable(
                "store maintenance is not configured",
            ));
        };
        maintenance.checkpoint(request.kind).await?;
        Ok(CheckpointStoreResponse { applied: true })
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
        AdminApi, AdminIdentity, AdminScope, CheckpointKind, CheckpointStoreRequest,
        DiagnosticsRequest, DrainNodeRequest, ReconcileScope, RotateTlsRequest, TlsComponent,
        TriggerReconciliationRequest,
    };
    use cheetah_media_api::capacity::CapacityLimits;
    use cheetah_media_api::error::MediaErrorCode;
    use cheetah_media_api::fencing::{LeaseStatus, MediaNodeLease, NodeRuntimeState, NodeState};
    use cheetah_media_api::ids::{MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId};
    use cheetah_runtime_tokio::TokioRuntime;

    use crate::capacity::CapacityOrchestrator;
    use crate::facade::{ControlPlane, TlsRotator};
    use crate::sqlite::SqliteStore;
    use crate::store::StoreMaintenance;

    use super::*;

    fn runtime() -> Arc<TokioRuntime> {
        Arc::new(TokioRuntime::new())
    }

    async fn control_plane() -> ControlPlane {
        let runtime = runtime();
        let store = SqliteStore::new(runtime.clone(), ":memory:").await.unwrap();
        let capacity = Arc::new(CapacityOrchestrator::new(CapacityLimits {
            session_count: 100,
            port_count: 100,
            bandwidth_bps: u64::MAX,
            worker_count: 100,
            blocking_job_count: 100,
            file_task_count: 100,
            event_subscriber_count: 100,
            cpu_permille: 1000,
        }));
        ControlPlane::new(
            runtime,
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_store_maintenance(Arc::new(store) as Arc<dyn StoreMaintenance>)
        .with_capacity(capacity)
    }

    fn sample_node() -> NodeRuntimeState {
        NodeRuntimeState {
            node_id: MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            instance_id: MediaNodeInstanceId::new("550e8401-e29b-41d4-a716-446655440000").unwrap(),
            accepted_instance_epoch: MediaNodeInstanceEpoch(1),
            state: NodeState::Active,
            lease: MediaNodeLease {
                lease_id: "lease-1".to_string(),
                status: LeaseStatus::Active,
                deadline_ms: i64::MAX,
                heartbeat_interval_ms: 5_000,
                cluster_time_ms: 0,
                accepted_contract_version: "v1".to_string(),
                accepted_instance_epoch: MediaNodeInstanceEpoch(1),
            },
            accepted_contract_version: "v1".to_string(),
            control_endpoint: "https://node:50051".to_string(),
            network_zone: None,
            region: None,
            labels: Default::default(),
            advertised_media_addresses: vec![],
            build_version: "0.1.0".to_string(),
            capability_generation: 1,
        }
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
    async fn trigger_reconciliation_validates_scope_fields() {
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
        assert_eq!(err.code, MediaErrorCode::InvalidArgument);

        let tenant = cheetah_media_api::ids::TenantId::new("tenant-a").unwrap();
        let scoped = TriggerReconciliationRequest {
            scope: ReconcileScope::Tenant,
            node_id: None,
            tenant_id: Some(tenant),
        };
        let resp = cp.trigger_reconciliation(&identity, scoped).await.unwrap();
        assert!(resp.triggered);
    }

    #[tokio::test]
    async fn drain_node_closes_capacity_gate() {
        let cp = control_plane().await;
        cp.set_node_runtime(Some(sample_node()));
        let identity = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Node],
        };
        let req = DrainNodeRequest {
            node_id: sample_node().node_id,
            drain: true,
            reason: "rolling restart".to_string(),
        };
        let resp = cp.drain_node(&identity, req).await.unwrap();
        assert!(resp.draining);
        assert_eq!(cp.node_runtime().unwrap().state, NodeState::Draining);
        assert!(
            !cp.capacity
                .as_ref()
                .unwrap()
                .snapshot()
                .await
                .unwrap()
                .node_gate_open
        );
    }

    #[tokio::test]
    async fn diagnostics_and_checkpoint_require_store_wiring() {
        let cp = control_plane().await;
        let reconcile_id = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Reconcile],
        };
        let store_id = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Store],
        };

        let diag = cp
            .inspect_diagnostics(
                &reconcile_id,
                DiagnosticsRequest {
                    tenant_id: None,
                    resource_kind: None,
                    max_events: 10,
                },
            )
            .await
            .unwrap();
        assert_eq!(diag.resource_count, 0);
        assert_eq!(diag.node_count, 0);

        let ck = cp
            .checkpoint_store(
                &store_id,
                CheckpointStoreRequest {
                    kind: CheckpointKind::Checkpoint,
                },
            )
            .await
            .unwrap();
        assert!(ck.applied);
    }

    struct FakeTlsRotator;

    #[async_trait]
    impl TlsRotator for FakeTlsRotator {
        async fn rotate(
            &self,
            _component: TlsComponent,
        ) -> Result<bool, cheetah_media_api::error::MediaError> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn rotate_tls_uses_configured_rotator() {
        let cp = control_plane()
            .await
            .with_tls_rotator(Arc::new(FakeTlsRotator));
        let identity = AdminIdentity {
            common_name: "ops".to_string(),
            scopes: vec![AdminScope::Tls],
        };
        let resp = cp
            .rotate_tls(
                &identity,
                RotateTlsRequest {
                    component: TlsComponent::GrpcListener,
                },
            )
            .await
            .unwrap();
        assert!(resp.applied);
    }
}
