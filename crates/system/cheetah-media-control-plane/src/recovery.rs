//! Startup recovery engine.
//!
//! 启动恢复引擎：在 gRPC serving 之前扫描非终态资源与 PREPARED/UNKNOWN 幂等记录，
//! 通过 typed provider probe 收敛状态，并补写缺失事件。

use std::sync::Arc;

use async_trait::async_trait;
use cheetah_media_api::controlled_event::{EventId, ResourceStateChanged};
use cheetah_media_api::error::{EffectOutcome, MediaError, MediaErrorCode};
use cheetah_media_api::fencing::ControlledResourceRef;
use cheetah_media_api::ids::ResourceGeneration;
use cheetah_media_api::resource_filter::ResourceState;

use crate::error::ControlPlaneError;
use crate::event_store::{EventRecord, EventStore};
use crate::idempotency::IdempotencyState;
use crate::store::{now_ms, IdempotencyRecord, IdempotencyStore, ResourceRecord, ResourceStore};

/// Bounds for a single recovery pass.
///
/// 单次恢复上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryLimits {
    /// Maximum non-terminal resources to inspect.
    pub max_resources: u32,
    /// Maximum PREPARED/UNKNOWN idempotency records to inspect.
    pub max_idempotency: u32,
    /// Time budget in milliseconds for the entire recovery pass. The deadline
    /// is computed from the start of `recover()`.
    pub deadline_ms: i64,
}

impl Default for RecoveryLimits {
    fn default() -> Self {
        Self {
            max_resources: 1000,
            max_idempotency: 1000,
            deadline_ms: 30_000,
        }
    }
}

/// Result of a startup recovery pass.
///
/// 启动恢复结果统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryReport {
    pub resources_scanned: u64,
    pub resources_converged: u64,
    pub resources_failed: u64,
    pub resources_unknown: u64,
    pub idempotency_scanned: u64,
    pub idempotency_converged: u64,
    pub elapsed_ms: i64,
}

/// Outcome of probing a resource through its typed provider.
///
/// provider 探测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// Provider confirms the resource exists in this state and generation.
    Found {
        state: ResourceState,
        generation: ResourceGeneration,
    },
    /// Provider confirms the resource no longer exists.
    Gone,
    /// Provider cannot determine the state; keep the record for reconciliation.
    Unknown,
}

/// Result of converging a stored resource record with a provider view.
///
/// 资源对账结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergeOutcome {
    /// No durable change was needed.
    Unchanged,
    /// The resource record was advanced to a known state.
    Converged,
    /// The provider view is stale or ambiguous; the record is marked Unknown.
    Ambiguous,
}

/// Port used by the recovery engine to ask a typed provider for the actual
/// state of a controlled resource.
///
/// 恢复引擎向 typed provider 查询资源实际状态的 port。
#[async_trait]
pub trait ResourceProbe: Send + Sync {
    /// Probe the provider for the resource identified by `resource_ref`.
    async fn probe(
        &self,
        resource_ref: &ControlledResourceRef,
    ) -> Result<ProbeResult, ControlPlaneError>;
}

/// Drives startup recovery by reconciling the durable index with typed
/// providers and backfilling missing events.
///
/// 驱动启动恢复：用 typed provider 对账持久索引并补写缺失事件。
pub struct RecoveryEngine<R, I, E, P> {
    resources: Arc<R>,
    idempotency: Arc<I>,
    events: Arc<E>,
    probe: Arc<P>,
}

impl<R, I, E, P> RecoveryEngine<R, I, E, P>
where
    R: ResourceStore,
    I: IdempotencyStore,
    E: EventStore,
    P: ResourceProbe,
{
    pub fn new(resources: Arc<R>, idempotency: Arc<I>, events: Arc<E>, probe: Arc<P>) -> Self {
        Self {
            resources,
            idempotency,
            events,
            probe,
        }
    }

    /// Run one recovery pass within `limits`.
    ///
    /// 在 `limits` 内执行一次恢复遍历。
    pub async fn recover(
        &self,
        limits: &RecoveryLimits,
    ) -> Result<RecoveryReport, ControlPlaneError> {
        let started_at = now_ms();
        let deadline = started_at + limits.deadline_ms;
        let mut report = RecoveryReport {
            resources_scanned: 0,
            resources_converged: 0,
            resources_failed: 0,
            resources_unknown: 0,
            idempotency_scanned: 0,
            idempotency_converged: 0,
            elapsed_ms: 0,
        };

        let resources = self
            .resources
            .list_non_terminal(limits.max_resources)
            .await?;
        for resource in resources {
            if now_ms() > deadline {
                break;
            }
            report.resources_scanned += 1;
            let resource_ref = resource.resource_ref();
            match self.probe.probe(&resource_ref).await? {
                ProbeResult::Found { state, generation } => {
                    match self.converge_resource(&resource, state, generation).await? {
                        ConvergeOutcome::Converged => report.resources_converged += 1,
                        ConvergeOutcome::Ambiguous => report.resources_unknown += 1,
                        ConvergeOutcome::Unchanged => {}
                    }
                }
                ProbeResult::Gone => {
                    self.resources
                        .tombstone(
                            &resource.tenant_id,
                            &resource.resource_kind,
                            &resource.resource_handle,
                            ResourceState::Stopped,
                        )
                        .await?;
                    self.backfill_state_event(
                        &resource_ref,
                        resource.state,
                        ResourceState::Stopped,
                        None,
                        resource.generation,
                    )
                    .await?;
                    report.resources_failed += 1;
                }
                ProbeResult::Unknown => {
                    if resource.state != ResourceState::Unknown {
                        self.resources
                            .set_state(
                                &resource.tenant_id,
                                &resource.resource_kind,
                                &resource.resource_handle,
                                ResourceState::Unknown,
                            )
                            .await?;
                        self.backfill_state_event(
                            &resource_ref,
                            resource.state,
                            ResourceState::Unknown,
                            resource.safe_last_error.clone(),
                            resource.generation,
                        )
                        .await?;
                    }
                    report.resources_unknown += 1;
                }
            }
        }

        let idem = self
            .idempotency
            .list_prepared_unknown(limits.max_idempotency)
            .await?;
        for record in idem {
            if now_ms() > deadline {
                break;
            }
            report.idempotency_scanned += 1;
            if let Some(resource_ref) = &record.resource_ref {
                self.reconcile_idempotency(&record, resource_ref, &mut report)
                    .await?;
            } else {
                // No resource reference means we cannot safely determine the
                // side-effect outcome; leave it for the reconciler.
                let mut updated = record.clone();
                updated.state = IdempotencyState::Unknown;
                updated.updated_at_ms = now_ms();
                self.idempotency.complete(&updated).await?;
            }
        }

        report.elapsed_ms = now_ms() - started_at;
        Ok(report)
    }

    async fn converge_resource(
        &self,
        resource: &ResourceRecord,
        state: ResourceState,
        generation: ResourceGeneration,
    ) -> Result<ConvergeOutcome, ControlPlaneError> {
        let resource_ref = resource.resource_ref();

        let mut new_state = resource.state;
        let mut new_generation = resource.generation;
        let mut changed = false;
        let ambiguous = if generation < resource.generation {
            if resource.state != ResourceState::Unknown {
                self.resources
                    .set_state(
                        &resource.tenant_id,
                        &resource.resource_kind,
                        &resource.resource_handle,
                        ResourceState::Unknown,
                    )
                    .await?;
                new_state = ResourceState::Unknown;
                changed = true;
            }
            true
        } else if generation > resource.generation {
            let ok = self
                .resources
                .compare_and_set_generation(
                    &resource.tenant_id,
                    &resource.resource_kind,
                    &resource.resource_handle,
                    resource.generation,
                    generation,
                    state,
                )
                .await?;
            if ok {
                new_state = state;
                new_generation = generation;
                changed = true;
            } else {
                // Another writer changed the generation; fall back to set_state
                // and re-fetch to record the actual persisted generation.
                self.resources
                    .set_state(
                        &resource.tenant_id,
                        &resource.resource_kind,
                        &resource.resource_handle,
                        state,
                    )
                    .await?;
                let updated = self
                    .resources
                    .get(
                        &resource.tenant_id,
                        &resource.resource_kind,
                        &resource.resource_handle,
                    )
                    .await?
                    .ok_or_else(|| {
                        ControlPlaneError::NotFound(
                            "controlled resource disappeared during convergence".to_string(),
                        )
                    })?;
                new_state = updated.state;
                new_generation = updated.generation;
                changed = new_state != resource.state || new_generation != resource.generation;
            }
            false
        } else if state != resource.state {
            self.resources
                .set_state(
                    &resource.tenant_id,
                    &resource.resource_kind,
                    &resource.resource_handle,
                    state,
                )
                .await?;
            new_state = state;
            changed = true;
            false
        } else {
            false
        };

        if changed {
            self.backfill_state_event(
                &resource_ref,
                resource.state,
                new_state,
                resource.safe_last_error.clone(),
                new_generation,
            )
            .await?;
        }

        Ok(if ambiguous {
            ConvergeOutcome::Ambiguous
        } else if changed {
            ConvergeOutcome::Converged
        } else {
            ConvergeOutcome::Unchanged
        })
    }

    async fn reconcile_idempotency(
        &self,
        record: &IdempotencyRecord,
        resource_ref: &ControlledResourceRef,
        report: &mut RecoveryReport,
    ) -> Result<(), ControlPlaneError> {
        match self.probe.probe(resource_ref).await? {
            ProbeResult::Found { state, generation } => {
                let outcome = if let Some(existing) = self
                    .resources
                    .get(
                        &resource_ref.tenant_id,
                        &resource_ref.resource_kind,
                        &resource_ref.resource_handle,
                    )
                    .await?
                {
                    self.converge_resource(&existing, state, generation).await?
                } else {
                    let recovered = ResourceRecord {
                        tenant_id: resource_ref.tenant_id.clone(),
                        resource_kind: resource_ref.resource_kind.clone(),
                        resource_handle: resource_ref.resource_handle.clone(),
                        media_session_id: resource_ref.media_session_id.clone(),
                        media_binding_id: resource_ref.media_binding_id.clone(),
                        media_key: None,
                        idempotency_scope: Some(format!(
                            "{}/{}/{}",
                            record.key.tenant_id.as_str(),
                            record.key.operation_kind,
                            record.key.key
                        )),
                        canonical_digest: Some(record.canonical_digest),
                        accepted_owner_epoch: resource_ref.owner_epoch,
                        media_node_id: None,
                        media_node_instance_id: None,
                        media_node_instance_epoch: resource_ref.node_instance_epoch,
                        generation,
                        state,
                        safe_last_error: None,
                        created_at_ms: now_ms(),
                        updated_at_ms: now_ms(),
                        terminal_at_ms: None,
                    };
                    self.resources.insert(&recovered).await?;
                    self.backfill_state_event(
                        resource_ref,
                        ResourceState::Pending,
                        state,
                        None,
                        generation,
                    )
                    .await?;
                    ConvergeOutcome::Converged
                };

                let mut completed = record.clone();
                if outcome == ConvergeOutcome::Ambiguous {
                    completed.state = IdempotencyState::Unknown;
                    completed.effect_outcome = EffectOutcome::Unknown;
                } else {
                    completed.state = IdempotencyState::Completed;
                    completed.effect_outcome = EffectOutcome::Applied;
                }
                completed.updated_at_ms = now_ms();
                self.idempotency.complete(&completed).await?;
                report.idempotency_converged += 1;
            }
            ProbeResult::Gone => {
                let mut completed = record.clone();
                completed.state = IdempotencyState::Failed;
                completed.effect_outcome = EffectOutcome::NotApplied;
                completed.safe_error = Some(MediaError::new(
                    MediaErrorCode::NotFound,
                    "resource gone during startup recovery",
                ));
                completed.updated_at_ms = now_ms();
                self.idempotency.complete(&completed).await?;
                report.idempotency_converged += 1;
            }
            ProbeResult::Unknown => {
                let mut updated = record.clone();
                updated.state = IdempotencyState::Unknown;
                updated.updated_at_ms = now_ms();
                self.idempotency.complete(&updated).await?;
            }
        }
        Ok(())
    }

    async fn backfill_state_event(
        &self,
        resource_ref: &ControlledResourceRef,
        previous_state: ResourceState,
        new_state: ResourceState,
        last_error: Option<MediaError>,
        generation: ResourceGeneration,
    ) -> Result<(), ControlPlaneError> {
        let sequence = self
            .events
            .next_sequence(resource_ref.node_instance_epoch)
            .await?;
        let payload = ResourceStateChanged {
            resource_kind: resource_ref.resource_kind.clone(),
            resource_handle: resource_ref.resource_handle.clone(),
            media_session_id: resource_ref.media_session_id.clone(),
            media_binding_id: resource_ref.media_binding_id.clone(),
            previous_state,
            new_state,
            owner_epoch: resource_ref.owner_epoch,
            generation,
            media_key: None,
            last_error,
        };
        let serialized = serde_json::to_string(&payload)
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?;
        let event = EventRecord {
            event_id: EventId::new(format!(
                "evt-{}-{}",
                resource_ref.node_instance_epoch.0, sequence.0
            ))
            .map_err(|e| ControlPlaneError::Serialization(e.to_string()))?,
            instance_epoch: resource_ref.node_instance_epoch,
            sequence,
            tenant_id: resource_ref.tenant_id.clone(),
            resource_kind: Some(resource_ref.resource_kind.clone()),
            resource_handle: Some(resource_ref.resource_handle.clone()),
            occurred_at: now_ms(),
            event_kind: "resource_state_changed".to_string(),
            serialized_payload: serialized,
            correlation_id: None,
            traceparent: None,
            tracestate: None,
            expires_at: now_ms() + 86_400_000,
        };
        self.events.append(&event).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cheetah_media_api::controlled_event::{EventSequence, ResourceStateChanged};
    use cheetah_media_api::error::{EffectOutcome, MediaErrorCode};
    use cheetah_media_api::fencing::ControlledResourceRef;
    use cheetah_media_api::ids::{
        MediaBindingId, MediaNodeInstanceEpoch, MediaSessionId, OwnerEpoch, ResourceGeneration,
        TenantId,
    };
    use cheetah_media_api::resource_filter::ResourceState;
    use cheetah_runtime_tokio::TokioRuntime;

    use super::*;
    use crate::event_store::EventStore;
    use crate::idempotency::{CanonicalDigest, IdempotencyKey, IdempotencyState};
    use crate::recovery::{ProbeResult, RecoveryEngine, RecoveryLimits, ResourceProbe};
    use crate::sqlite::SqliteStore;
    use crate::store::{IdempotencyRecord, ResourceRecord, ResourceStore};

    struct FakeProbe {
        results: std::collections::HashMap<String, ProbeResult>,
    }

    #[async_trait]
    impl ResourceProbe for FakeProbe {
        async fn probe(
            &self,
            resource_ref: &ControlledResourceRef,
        ) -> Result<ProbeResult, ControlPlaneError> {
            Ok(*self
                .results
                .get(&resource_ref.resource_handle)
                .unwrap_or(&ProbeResult::Unknown))
        }
    }

    fn sample_resource(tenant: &TenantId, handle: &str) -> ResourceRecord {
        let session = MediaSessionId::new("550e8400-e29b-41d4-a716-446655440001").unwrap();
        let binding = MediaBindingId::new("550e8400-e29b-41d4-a716-446655440002").unwrap();
        ResourceRecord {
            tenant_id: tenant.clone(),
            resource_kind: "publisher".to_string(),
            resource_handle: handle.to_string(),
            media_session_id: Some(session),
            media_binding_id: Some(binding),
            media_key: None,
            idempotency_scope: None,
            canonical_digest: None,
            accepted_owner_epoch: OwnerEpoch(1),
            media_node_id: None,
            media_node_instance_id: None,
            media_node_instance_epoch: MediaNodeInstanceEpoch(10),
            generation: ResourceGeneration(0),
            state: ResourceState::Active,
            safe_last_error: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            terminal_at_ms: None,
        }
    }

    fn resource_ref(tenant: &TenantId, handle: &str) -> ControlledResourceRef {
        ControlledResourceRef {
            tenant_id: tenant.clone(),
            media_session_id: None,
            media_binding_id: None,
            resource_kind: "publisher".to_string(),
            resource_handle: handle.to_string(),
            owner_epoch: OwnerEpoch(1),
            node_instance_epoch: MediaNodeInstanceEpoch(10),
            generation: ResourceGeneration(0),
            origin: Default::default(),
        }
    }

    #[tokio::test]
    async fn recovery_converges_and_backfills_event() {
        let rt = Arc::new(TokioRuntime::new());
        let store = Arc::new(SqliteStore::new(rt, ":memory:").await.unwrap());

        let tenant = TenantId::new("tenant-1").unwrap();
        let resource = sample_resource(&tenant, "pub-1");
        store.insert(&resource).await.unwrap();

        let mut results = std::collections::HashMap::new();
        results.insert(
            "pub-1".to_string(),
            ProbeResult::Found {
                state: ResourceState::Active,
                generation: ResourceGeneration(1),
            },
        );
        let probe = Arc::new(FakeProbe { results });

        let engine = RecoveryEngine::new(store.clone(), store.clone(), store.clone(), probe);
        let report = engine.recover(&RecoveryLimits::default()).await.unwrap();

        assert_eq!(report.resources_scanned, 1);
        assert_eq!(report.resources_converged, 1);

        let loaded = ResourceStore::get(store.as_ref(), &tenant, "publisher", "pub-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.generation, ResourceGeneration(1));

        let events = EventStore::list_by_resource(
            store.as_ref(),
            &tenant,
            "publisher",
            "pub-1",
            MediaNodeInstanceEpoch(0),
            EventSequence(0),
            10,
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id.as_str(), "evt-10-1");
        let payload: ResourceStateChanged =
            serde_json::from_str(&events[0].serialized_payload).unwrap();
        assert_eq!(payload.previous_state, ResourceState::Active);
        assert_eq!(payload.new_state, ResourceState::Active);
        assert_eq!(payload.generation, ResourceGeneration(1));
    }

    #[tokio::test]
    async fn recovery_does_not_backfill_unchanged_resource() {
        let rt = Arc::new(TokioRuntime::new());
        let store = Arc::new(SqliteStore::new(rt, ":memory:").await.unwrap());

        let tenant = TenantId::new("tenant-1").unwrap();
        let resource = sample_resource(&tenant, "pub-same");
        store.insert(&resource).await.unwrap();

        let mut results = std::collections::HashMap::new();
        results.insert(
            "pub-same".to_string(),
            ProbeResult::Found {
                state: ResourceState::Active,
                generation: ResourceGeneration(0),
            },
        );
        let probe = Arc::new(FakeProbe { results });

        let engine = RecoveryEngine::new(store.clone(), store.clone(), store.clone(), probe);
        let report = engine.recover(&RecoveryLimits::default()).await.unwrap();

        assert_eq!(report.resources_scanned, 1);
        assert_eq!(report.resources_converged, 0);

        let events = EventStore::list_by_resource(
            store.as_ref(),
            &tenant,
            "publisher",
            "pub-same",
            MediaNodeInstanceEpoch(0),
            EventSequence(0),
            10,
        )
        .await
        .unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn recovery_tombstones_gone_resource() {
        let rt = Arc::new(TokioRuntime::new());
        let store = Arc::new(SqliteStore::new(rt, ":memory:").await.unwrap());

        let tenant = TenantId::new("tenant-1").unwrap();
        let resource = sample_resource(&tenant, "pub-gone");
        store.insert(&resource).await.unwrap();

        let mut results = std::collections::HashMap::new();
        results.insert("pub-gone".to_string(), ProbeResult::Gone);
        let probe = Arc::new(FakeProbe { results });

        let engine = RecoveryEngine::new(store.clone(), store.clone(), store.clone(), probe);
        let report = engine.recover(&RecoveryLimits::default()).await.unwrap();

        assert_eq!(report.resources_failed, 1);

        let loaded = ResourceStore::get(store.as_ref(), &tenant, "publisher", "pub-gone")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ResourceState::Stopped);
        assert!(loaded.terminal_at_ms.is_some());
    }

    #[tokio::test]
    async fn recovery_completes_prepared_idempotency_when_resource_found() {
        let rt = Arc::new(TokioRuntime::new());
        let store = Arc::new(SqliteStore::new(rt, ":memory:").await.unwrap());

        let tenant = TenantId::new("tenant-1").unwrap();
        let key = IdempotencyKey::new(tenant.clone(), "create_publisher", "key-1");
        let digest = CanonicalDigest([1u8; 32]);
        let r#ref = resource_ref(&tenant, "pub-idem");

        let idem = IdempotencyRecord {
            key: key.clone(),
            state: IdempotencyState::Prepared,
            canonical_digest: digest,
            resource_ref: Some(r#ref.clone()),
            effect_outcome: EffectOutcome::Unknown,
            serialized_domain_result: None,
            safe_error: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            expires_at_ms: now_ms() + 60_000,
            attempt_count: 0,
        };
        store.complete(&idem).await.unwrap();

        let mut results = std::collections::HashMap::new();
        results.insert(
            "pub-idem".to_string(),
            ProbeResult::Found {
                state: ResourceState::Active,
                generation: ResourceGeneration(2),
            },
        );
        let probe = Arc::new(FakeProbe { results });

        let engine = RecoveryEngine::new(store.clone(), store.clone(), store.clone(), probe);
        let report = engine.recover(&RecoveryLimits::default()).await.unwrap();

        assert_eq!(report.idempotency_converged, 1);

        let loaded = ResourceStore::get(store.as_ref(), &tenant, "publisher", "pub-idem")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ResourceState::Active);
        assert_eq!(loaded.generation, ResourceGeneration(2));

        let completed = IdempotencyStore::get(store.as_ref(), &key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.state, IdempotencyState::Completed);
    }

    #[tokio::test]
    async fn recovery_fails_prepared_idempotency_when_resource_gone() {
        let rt = Arc::new(TokioRuntime::new());
        let store = Arc::new(SqliteStore::new(rt, ":memory:").await.unwrap());

        let tenant = TenantId::new("tenant-1").unwrap();
        let key = IdempotencyKey::new(tenant.clone(), "create_publisher", "key-2");
        let digest = CanonicalDigest([2u8; 32]);
        let r#ref = resource_ref(&tenant, "pub-gone-2");

        let idem = IdempotencyRecord {
            key: key.clone(),
            state: IdempotencyState::Prepared,
            canonical_digest: digest,
            resource_ref: Some(r#ref),
            effect_outcome: EffectOutcome::Unknown,
            serialized_domain_result: None,
            safe_error: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            expires_at_ms: now_ms() + 60_000,
            attempt_count: 0,
        };
        store.complete(&idem).await.unwrap();

        let mut results = std::collections::HashMap::new();
        results.insert("pub-gone-2".to_string(), ProbeResult::Gone);
        let probe = Arc::new(FakeProbe { results });

        let engine = RecoveryEngine::new(store.clone(), store.clone(), store.clone(), probe);
        let report = engine.recover(&RecoveryLimits::default()).await.unwrap();

        assert_eq!(report.idempotency_converged, 1);

        let completed = IdempotencyStore::get(store.as_ref(), &key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.state, IdempotencyState::Failed);
        assert_eq!(
            completed.safe_error.as_ref().map(|e| e.code),
            Some(MediaErrorCode::NotFound)
        );
    }

    #[tokio::test]
    async fn recovery_backfills_unknown_transition() {
        let rt = Arc::new(TokioRuntime::new());
        let store = Arc::new(SqliteStore::new(rt, ":memory:").await.unwrap());

        let tenant = TenantId::new("tenant-1").unwrap();
        let resource = sample_resource(&tenant, "pub-unknown");
        store.insert(&resource).await.unwrap();

        let mut results = std::collections::HashMap::new();
        results.insert("pub-unknown".to_string(), ProbeResult::Unknown);
        let probe = Arc::new(FakeProbe { results });

        let engine = RecoveryEngine::new(store.clone(), store.clone(), store.clone(), probe);
        let report = engine.recover(&RecoveryLimits::default()).await.unwrap();

        assert_eq!(report.resources_unknown, 1);

        let loaded = ResourceStore::get(store.as_ref(), &tenant, "publisher", "pub-unknown")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ResourceState::Unknown);

        let events = EventStore::list_by_resource(
            store.as_ref(),
            &tenant,
            "publisher",
            "pub-unknown",
            MediaNodeInstanceEpoch(0),
            EventSequence(0),
            10,
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 1);
        let payload: ResourceStateChanged =
            serde_json::from_str(&events[0].serialized_payload).unwrap();
        assert_eq!(payload.previous_state, ResourceState::Active);
        assert_eq!(payload.new_state, ResourceState::Unknown);
    }
}
