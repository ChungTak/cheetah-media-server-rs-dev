//! Idempotent mutation orchestration (IDEM-02).
//!
//! Coordinates node gate, capacity, idempotency prepare/complete, and resource
//! registration so adapters do not re-implement the crash-window state machine.
//!
//! 幂等变更编排：节点门控、容量、幂等 prepare/complete 与资源登记。

use std::sync::Arc;

use cheetah_media_api::error::{EffectOutcome, MediaError, MediaErrorCode};
use cheetah_media_api::fencing::ControlledResourceRef;
use serde_json::Value;

use crate::capacity::CapacityOrchestrator;
use crate::error::ControlPlaneError;
use crate::idempotency::{CanonicalDigest, IdempotencyKey, IdempotencyState};
use crate::node_supervisor::NodeSupervisor;
use crate::store::{
    now_ms, IdempotencyOutcome, IdempotencyRecord, IdempotencyStore, ResourceRecord, ResourceStore,
};
use cheetah_media_api::port::MediaCapacityApi;

/// Result of preparing an idempotent mutation.
///
/// 幂等变更 prepare 的结果。
#[derive(Debug, Clone)]
pub enum MutationPrepareResult {
    /// Caller should execute the side effect.
    Proceed {
        key: IdempotencyKey,
        digest: CanonicalDigest,
    },
    /// Replay a previously completed/failed result.
    Replay(Box<IdempotencyRecord>),
    /// Same key, different digest.
    Conflict,
    /// PREPARED/UNKNOWN — must reconcile, never blind recreate.
    Reconcile(Box<IdempotencyRecord>),
}

/// Orchestrates the durable mutation protocol.
pub struct MutationOrchestrator {
    idempotency: Arc<dyn IdempotencyStore>,
    resources: Arc<dyn ResourceStore>,
    capacity: Option<Arc<CapacityOrchestrator>>,
    node: Option<Arc<NodeSupervisor>>,
}

impl MutationOrchestrator {
    pub fn new(idempotency: Arc<dyn IdempotencyStore>, resources: Arc<dyn ResourceStore>) -> Self {
        Self {
            idempotency,
            resources,
            capacity: None,
            node: None,
        }
    }

    pub fn with_capacity(mut self, capacity: Arc<CapacityOrchestrator>) -> Self {
        self.capacity = Some(capacity);
        self
    }

    pub fn with_node(mut self, node: Arc<NodeSupervisor>) -> Self {
        self.node = Some(node);
        self
    }

    /// Prepare an idempotent mutation after node/capacity gates.
    ///
    /// Rejects with `Busy`/`Unavailable` + `NotApplied` when the create gate is
    /// closed or the node is not Active.
    pub async fn prepare(
        &self,
        key: &IdempotencyKey,
        digest: CanonicalDigest,
        expires_at_ms: i64,
    ) -> Result<MutationPrepareResult, ControlPlaneError> {
        key.validate()?;

        if let Some(node) = &self.node {
            if !node.mutations_allowed() {
                return Err(ControlPlaneError::Media(
                    MediaError::new(
                        MediaErrorCode::Busy,
                        format!("node is not accepting mutations (state={:?})", node.state()),
                    )
                    .with_retryable(true)
                    .with_retry_after(100),
                ));
            }
        }

        if let Some(capacity) = &self.capacity {
            let snap = capacity.snapshot().await?;
            if !snap.node_gate_open {
                return Err(ControlPlaneError::Media(
                    MediaError::new(MediaErrorCode::Busy, "node create gate is closed")
                        .with_retryable(true)
                        .with_retry_after(100),
                ));
            }
        }

        match self.idempotency.prepare(key, digest, expires_at_ms).await? {
            IdempotencyOutcome::Proceed => Ok(MutationPrepareResult::Proceed {
                key: key.clone(),
                digest,
            }),
            IdempotencyOutcome::Replay(record) => Ok(MutationPrepareResult::Replay(record)),
            IdempotencyOutcome::Conflict => Ok(MutationPrepareResult::Conflict),
            IdempotencyOutcome::Reconcile => {
                let record = self.idempotency.get(key).await?.ok_or_else(|| {
                    ControlPlaneError::Internal(
                        "idempotency reconcile without stored record".to_string(),
                    )
                })?;
                Ok(MutationPrepareResult::Reconcile(Box::new(record)))
            }
        }
    }

    /// Persist a successful mutation outcome and optional resource record.
    ///
    /// Must be called only after the side effect has completed. The success
    /// response may only be sent after this returns Ok.
    pub async fn complete_success(
        &self,
        key: &IdempotencyKey,
        digest: CanonicalDigest,
        resource: Option<&ResourceRecord>,
        domain_result: Option<Value>,
    ) -> Result<IdempotencyRecord, ControlPlaneError> {
        if let Some(record) = resource {
            // Insert if missing; Conflict on duplicate is treated as success when
            // the handle matches (response-loss replay path).
            match self.resources.insert(record).await {
                Ok(()) => {}
                Err(ControlPlaneError::Conflict(_)) => {
                    // Verify the existing record matches the handle.
                    let existing = self
                        .resources
                        .get(
                            &record.tenant_id,
                            &record.resource_kind,
                            &record.resource_handle,
                        )
                        .await?;
                    if existing.is_none() {
                        return Err(ControlPlaneError::Conflict(
                            "resource insert conflict without existing row".to_string(),
                        ));
                    }
                }
                Err(e) => return Err(e),
            }
        }

        let resource_ref = resource.map(ResourceRecord::resource_ref);
        let now = now_ms();
        let record = IdempotencyRecord {
            key: key.clone(),
            state: IdempotencyState::Completed,
            canonical_digest: digest,
            resource_ref,
            effect_outcome: EffectOutcome::Applied,
            serialized_domain_result: domain_result,
            safe_error: None,
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: now.saturating_add(24 * 60 * 60 * 1000),
            attempt_count: 1,
        };
        self.idempotency.complete(&record).await?;
        Ok(record)
    }

    /// Persist a failed mutation. `outcome` must reflect whether side effects remain.
    pub async fn complete_failure(
        &self,
        key: &IdempotencyKey,
        digest: CanonicalDigest,
        error: MediaError,
        outcome: EffectOutcome,
        resource_ref: Option<ControlledResourceRef>,
    ) -> Result<IdempotencyRecord, ControlPlaneError> {
        // UNKNOWN failures must not be auto-retried; store as Unknown state.
        let state = if matches!(outcome, EffectOutcome::Unknown) {
            IdempotencyState::Unknown
        } else {
            IdempotencyState::Failed
        };
        let now = now_ms();
        let record = IdempotencyRecord {
            key: key.clone(),
            state,
            canonical_digest: digest,
            resource_ref,
            effect_outcome: outcome,
            serialized_domain_result: None,
            safe_error: Some(error),
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: now.saturating_add(24 * 60 * 60 * 1000),
            attempt_count: 1,
        };
        self.idempotency.complete(&record).await?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capacity::CapacityOrchestrator;
    use crate::idempotency::CanonicalRequest;
    use crate::node_supervisor::{CapacityLoadProvider, FakeClock, NodeSupervisor, RegistryClient};
    use crate::sqlite::SqliteStore;
    use cheetah_media_api::capacity::CapacityLimits;
    use cheetah_media_api::fencing::{LeaseStatus, MediaNodeLease, ResourceOrigin};
    use cheetah_media_api::ids::{
        MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId, OwnerEpoch, ResourceGeneration,
        TenantId,
    };
    use cheetah_media_api::node::{
        NodeDeregisterRequest, NodeDeregisterResponse, NodeHeartbeat, NodeHeartbeatResponse,
        NodeIdentity, NodeRegistrationRequest, NodeRegistrationResponse,
    };
    use cheetah_media_api::resource_filter::ResourceState;
    use cheetah_runtime_tokio::TokioRuntime;
    use std::collections::HashMap;

    struct OkRegistry;

    #[async_trait::async_trait]
    impl RegistryClient for OkRegistry {
        async fn register(
            &self,
            _request: NodeRegistrationRequest,
        ) -> Result<NodeRegistrationResponse, ControlPlaneError> {
            let epoch = MediaNodeInstanceEpoch(1);
            Ok(NodeRegistrationResponse {
                instance_epoch: epoch,
                lease: MediaNodeLease {
                    lease_id: "lease-1".to_string(),
                    status: LeaseStatus::Active,
                    deadline_ms: i64::MAX,
                    heartbeat_interval_ms: 5_000,
                    cluster_time_ms: 0,
                    accepted_contract_version: "v1".to_string(),
                    accepted_instance_epoch: epoch,
                },
                accepted_contract_version: "v1".to_string(),
                cluster_time_ms: 0,
            })
        }

        async fn heartbeat(
            &self,
            _heartbeat: NodeHeartbeat,
        ) -> Result<NodeHeartbeatResponse, ControlPlaneError> {
            Ok(NodeHeartbeatResponse {
                lease: None,
                next_heartbeat_interval_ms: 5_000,
            })
        }

        async fn deregister(
            &self,
            _request: NodeDeregisterRequest,
        ) -> Result<NodeDeregisterResponse, ControlPlaneError> {
            Ok(NodeDeregisterResponse { acknowledged: true })
        }
    }

    fn identity() -> NodeIdentity {
        NodeIdentity {
            node_id: MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            instance_id: MediaNodeInstanceId::new("550e8401-e29b-41d4-a716-446655440001").unwrap(),
            instance_epoch: MediaNodeInstanceEpoch(0),
            control_endpoint: "https://node:50051".to_string(),
            network_zone: None,
            region: None,
            labels: HashMap::new(),
            advertised_media_addresses: vec![],
            build_version: "0.1.0".to_string(),
            contract_range: ">=1".to_string(),
            contract_checksum: "sha256:x".to_string(),
            capability_generation: 1,
        }
    }

    async fn store() -> SqliteStore {
        let rt = Arc::new(TokioRuntime::new());
        SqliteStore::new(rt, ":memory:").await.unwrap()
    }

    fn digest() -> CanonicalDigest {
        CanonicalRequest {
            schema_version: 1,
            tenant_id: TenantId::new("tenant-1").unwrap(),
            operation_kind: "create".to_string(),
            target: None,
            media_session_id: None,
            media_binding_id: None,
            business_params: serde_json::json!({"a": 1}),
        }
        .digest()
        .unwrap()
    }

    #[tokio::test]
    async fn prepare_proceed_then_complete_replays() {
        let store = store().await;
        let orch = MutationOrchestrator::new(Arc::new(store.clone()), Arc::new(store.clone()));
        let tenant = TenantId::new("tenant-1").unwrap();
        let key = IdempotencyKey::new(tenant.clone(), "create", "k1");
        let d = digest();

        let prep = orch.prepare(&key, d, now_ms() + 60_000).await.unwrap();
        assert!(matches!(prep, MutationPrepareResult::Proceed { .. }));

        let resource = ResourceRecord {
            tenant_id: tenant.clone(),
            resource_kind: "publisher".to_string(),
            resource_handle: "pub-1".to_string(),
            media_session_id: None,
            media_binding_id: None,
            media_key: None,
            idempotency_scope: Some("tenant-1/create/k1".to_string()),
            canonical_digest: Some(d),
            accepted_owner_epoch: OwnerEpoch(1),
            media_node_id: None,
            media_node_instance_id: None,
            media_node_instance_epoch: MediaNodeInstanceEpoch(1),
            generation: ResourceGeneration(0),
            state: ResourceState::Active,
            safe_last_error: None,
            origin: ResourceOrigin::Cluster,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            terminal_at_ms: None,
        };
        orch.complete_success(
            &key,
            d,
            Some(&resource),
            Some(serde_json::json!({"ok": true})),
        )
        .await
        .unwrap();

        let prep2 = orch.prepare(&key, d, now_ms() + 60_000).await.unwrap();
        match prep2 {
            MutationPrepareResult::Replay(rec) => {
                assert_eq!(rec.state, IdempotencyState::Completed);
                assert_eq!(rec.effect_outcome, EffectOutcome::Applied);
            }
            other => panic!("expected replay, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prepare_rejects_when_gate_closed() {
        let store = store().await;
        let capacity = Arc::new(CapacityOrchestrator::new(CapacityLimits {
            session_count: 10,
            port_count: 10,
            bandwidth_bps: u64::MAX,
            worker_count: 10,
            blocking_job_count: 10,
            file_task_count: 10,
            event_subscriber_count: 10,
            cpu_permille: 1000,
        }));
        capacity.set_node_gate(false).await.unwrap();
        let orch = MutationOrchestrator::new(Arc::new(store.clone()), Arc::new(store))
            .with_capacity(capacity);
        let key = IdempotencyKey::new(TenantId::new("t").unwrap(), "create", "k");
        let err = orch
            .prepare(&key, digest(), now_ms() + 60_000)
            .await
            .unwrap_err();
        assert_eq!(err.code(), MediaErrorCode::Busy);
    }

    #[tokio::test]
    async fn prepare_rejects_when_node_not_active() {
        let store = store().await;
        let capacity = Arc::new(CapacityOrchestrator::new(CapacityLimits {
            session_count: 10,
            port_count: 10,
            bandwidth_bps: u64::MAX,
            worker_count: 10,
            blocking_job_count: 10,
            file_task_count: 10,
            event_subscriber_count: 10,
            cpu_permille: 1000,
        }));
        let load = Arc::new(CapacityLoadProvider::new(capacity.clone()));
        let clock = Arc::new(FakeClock::new(1_000));
        let sup = Arc::new(NodeSupervisor::new(
            identity(),
            capacity.clone(),
            Arc::new(OkRegistry),
            load,
            clock,
        ));
        // Not registered => mutations not allowed.
        let orch = MutationOrchestrator::new(Arc::new(store.clone()), Arc::new(store))
            .with_capacity(capacity)
            .with_node(sup);
        let key = IdempotencyKey::new(TenantId::new("t").unwrap(), "create", "k");
        let err = orch
            .prepare(&key, digest(), now_ms() + 60_000)
            .await
            .unwrap_err();
        assert_eq!(err.code(), MediaErrorCode::Busy);
    }

    #[tokio::test]
    async fn different_digest_is_conflict() {
        let store = store().await;
        let orch = MutationOrchestrator::new(Arc::new(store.clone()), Arc::new(store));
        let key = IdempotencyKey::new(TenantId::new("t").unwrap(), "create", "k");
        let d1 = digest();
        orch.prepare(&key, d1, now_ms() + 60_000).await.unwrap();
        orch.complete_success(&key, d1, None, None).await.unwrap();

        let mut d2_bytes = d1.0;
        d2_bytes[0] ^= 0xff;
        let d2 = CanonicalDigest(d2_bytes);
        let prep = orch.prepare(&key, d2, now_ms() + 60_000).await.unwrap();
        assert!(matches!(prep, MutationPrepareResult::Conflict));
    }
}
