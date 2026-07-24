//! Node lifecycle supervisor: register, heartbeat, lease loss, drain, shutdown.
//!
//! Implements NODE-02..05 as a runtime-neutral state machine. The registry is
//! injected via `RegistryClient` so the control plane does not depend on tonic
//! or signaling DTO crates.
//!
//! 节点生命周期 supervisor：注册、心跳、租约丢失、drain 与关机。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use async_trait::async_trait;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::fencing::{
    LeaseLossReason, LeaseStatus, MediaNodeLease, NodeRuntimeState, NodeState,
};
use cheetah_media_api::node::{
    NodeDeregisterRequest, NodeDeregisterResponse, NodeDrainRequest, NodeDrainResponse,
    NodeHeartbeat, NodeHeartbeatResponse, NodeIdentity, NodeIsolateRequest, NodeIsolateResponse,
    NodeLoad, NodeRegistrationRequest, NodeRegistrationResponse,
};
use cheetah_media_api::port::MediaCapacityApi;

use crate::capacity::CapacityOrchestrator;
use crate::error::ControlPlaneError;

/// Clock abstraction so lease/heartbeat tests can use a fake clock.
///
/// 时钟抽象，便于租约/心跳测试注入 FakeClock。
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// System wall clock.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        crate::store::now_ms()
    }
}

/// Deterministic clock for tests.
#[derive(Debug, Default)]
pub struct FakeClock {
    now_ms: AtomicI64,
}

impl FakeClock {
    pub fn new(start_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(start_ms),
        }
    }

    pub fn advance(&self, delta_ms: i64) {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst);
    }

    pub fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// Outbound registry port used by the node supervisor.
///
/// 节点 supervisor 使用的出站注册中心 port。
#[async_trait]
pub trait RegistryClient: Send + Sync {
    async fn register(
        &self,
        request: NodeRegistrationRequest,
    ) -> Result<NodeRegistrationResponse, ControlPlaneError>;

    async fn heartbeat(
        &self,
        heartbeat: NodeHeartbeat,
    ) -> Result<NodeHeartbeatResponse, ControlPlaneError>;

    async fn deregister(
        &self,
        request: NodeDeregisterRequest,
    ) -> Result<NodeDeregisterResponse, ControlPlaneError>;
}

/// Supplies live load metrics for heartbeats.
///
/// 为心跳提供实时负载指标。
#[async_trait]
pub trait LoadProvider: Send + Sync {
    async fn current_load(&self, drain_state: NodeState) -> Result<NodeLoad, ControlPlaneError>;
}

/// Default load provider that derives usage from the capacity orchestrator.
///
/// 默认负载提供者：从容量编排器推导使用量。
pub struct CapacityLoadProvider {
    capacity: Arc<CapacityOrchestrator>,
}

impl CapacityLoadProvider {
    pub fn new(capacity: Arc<CapacityOrchestrator>) -> Self {
        Self { capacity }
    }
}

#[async_trait]
impl LoadProvider for CapacityLoadProvider {
    async fn current_load(&self, drain_state: NodeState) -> Result<NodeLoad, ControlPlaneError> {
        let snap = self.capacity.snapshot().await?;
        Ok(NodeLoad {
            session_count: snap.used.session_count,
            port_count: snap.used.port_count,
            bandwidth_bps: snap.used.bandwidth_bps,
            worker_count: snap.used.worker_count,
            blocking_job_count: snap.used.blocking_job_count,
            file_task_count: snap.used.file_task_count,
            event_subscriber_count: snap.used.event_subscriber_count,
            cpu_permille: snap.used.cpu_permille,
            degraded_reasons: Vec::new(),
            drain_state,
        })
    }
}

struct SupervisorInner {
    identity: NodeIdentity,
    runtime: Option<NodeRuntimeState>,
    /// Next heartbeat interval requested by the registry (ms).
    next_heartbeat_interval_ms: u64,
    /// Last successful registration attempt time.
    last_register_attempt_ms: i64,
    /// Register backoff base (ms).
    register_backoff_ms: u64,
    /// Drain hard deadline, if draining.
    drain_deadline_ms: Option<i64>,
}

/// Node lifecycle supervisor.
///
/// Holds fencing-critical runtime state and drives capacity create-gate
/// transitions. Does not spawn background tasks; the host runtime is expected
/// to call `tick` / `heartbeat` on a schedule.
///
/// 节点生命周期 supervisor。不启动后台任务；宿主按调度调用 `tick` / `heartbeat`。
pub struct NodeSupervisor {
    inner: Mutex<SupervisorInner>,
    capacity: Arc<CapacityOrchestrator>,
    registry: Arc<dyn RegistryClient>,
    load: Arc<dyn LoadProvider>,
    clock: Arc<dyn Clock>,
}

impl NodeSupervisor {
    /// Create a supervisor in `Disabled` with the given stable identity.
    pub fn new(
        identity: NodeIdentity,
        capacity: Arc<CapacityOrchestrator>,
        registry: Arc<dyn RegistryClient>,
        load: Arc<dyn LoadProvider>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner: Mutex::new(SupervisorInner {
                identity,
                runtime: None,
                next_heartbeat_interval_ms: 5_000,
                last_register_attempt_ms: 0,
                register_backoff_ms: 1_000,
                drain_deadline_ms: None,
            }),
            capacity,
            registry,
            load,
            clock,
        }
    }

    /// Snapshot the current node runtime state, if registered.
    pub fn runtime_state(&self) -> Option<NodeRuntimeState> {
        self.inner.lock().runtime.clone()
    }

    /// Current lifecycle state (Disabled when not yet registered).
    pub fn state(&self) -> NodeState {
        self.inner
            .lock()
            .runtime
            .as_ref()
            .map(|r| r.state)
            .unwrap_or(NodeState::Disabled)
    }

    /// Whether create/open/start/connect mutations are allowed.
    pub fn mutations_allowed(&self) -> bool {
        matches!(self.state(), NodeState::Active)
    }

    /// Whether tenant-scoped read/stop/delete is allowed.
    pub fn reads_allowed(&self) -> bool {
        matches!(
            self.state(),
            NodeState::Active | NodeState::Draining | NodeState::Isolated
        )
    }

    /// Heartbeat interval currently requested by the registry.
    pub fn heartbeat_interval_ms(&self) -> u64 {
        self.inner.lock().next_heartbeat_interval_ms
    }

    /// Begin registration: Binding -> Registering -> Active (or stay Registering on failure).
    pub async fn register(&self) -> Result<NodeRegistrationResponse, ControlPlaneError> {
        {
            let mut g = self.inner.lock();
            match g.runtime.as_ref().map(|r| r.state) {
                None | Some(NodeState::Disabled) | Some(NodeState::Isolated) => {
                    g.runtime = Some(placeholder_runtime(&g.identity, NodeState::Binding));
                }
                Some(NodeState::Active) | Some(NodeState::Draining) => {
                    return Err(ControlPlaneError::Conflict(
                        "node already registered".to_string(),
                    ));
                }
                Some(NodeState::Deregistering) | Some(NodeState::Stopped) => {
                    return Err(ControlPlaneError::Conflict(
                        "node is shutting down".to_string(),
                    ));
                }
                Some(NodeState::Binding) | Some(NodeState::Registering) => {}
            }
            if let Some(rt) = g.runtime.as_mut() {
                rt.state = NodeState::Registering;
            }
            g.last_register_attempt_ms = self.clock.now_ms();
        }

        // Close create gate until registration response is persisted.
        self.capacity.set_node_gate(false).await?;

        let request = {
            let g = self.inner.lock();
            NodeRegistrationRequest {
                node_identity: g.identity.clone(),
                previous_lease_id: g
                    .runtime
                    .as_ref()
                    .map(|r| r.lease.lease_id.clone())
                    .filter(|s| !s.is_empty()),
            }
        };

        match self.registry.register(request).await {
            Ok(resp) => {
                self.apply_registration(&resp).await?;
                Ok(resp)
            }
            Err(e) => {
                let mut g = self.inner.lock();
                // Stay in Registering so the host can retry with backoff.
                if let Some(rt) = g.runtime.as_mut() {
                    rt.state = NodeState::Registering;
                }
                g.register_backoff_ms = (g.register_backoff_ms.saturating_mul(2)).min(60_000);
                Err(e)
            }
        }
    }

    async fn apply_registration(
        &self,
        resp: &NodeRegistrationResponse,
    ) -> Result<(), ControlPlaneError> {
        {
            let mut g = self.inner.lock();
            g.identity.instance_epoch = resp.instance_epoch;
            let identity = g.identity.clone();
            g.runtime = Some(NodeRuntimeState {
                node_id: identity.node_id.clone(),
                instance_id: identity.instance_id.clone(),
                accepted_instance_epoch: resp.instance_epoch,
                state: NodeState::Active,
                lease: resp.lease.clone(),
                accepted_contract_version: resp.accepted_contract_version.clone(),
                control_endpoint: identity.control_endpoint.clone(),
                network_zone: identity.network_zone.clone(),
                region: identity.region.clone(),
                labels: identity.labels.clone(),
                advertised_media_addresses: identity.advertised_media_addresses.clone(),
                build_version: identity.build_version.clone(),
                capability_generation: identity.capability_generation,
            });
            g.next_heartbeat_interval_ms = resp.lease.heartbeat_interval_ms.max(100);
            g.register_backoff_ms = 1_000;
            g.drain_deadline_ms = None;
        }
        // Only open the create gate after the registration response is applied.
        self.capacity.set_node_gate(true).await?;
        Ok(())
    }

    /// Send one heartbeat if the node is Active or Draining.
    pub async fn heartbeat(&self) -> Result<NodeHeartbeatResponse, ControlPlaneError> {
        let (mut hb, expected_epoch, expected_lease) = {
            let g = self.inner.lock();
            let rt = g.runtime.as_ref().ok_or_else(|| {
                ControlPlaneError::Media(MediaError::unavailable(
                    "node is not registered".to_string(),
                ))
            })?;
            if !matches!(rt.state, NodeState::Active | NodeState::Draining) {
                return Err(ControlPlaneError::Media(MediaError::unavailable(format!(
                    "heartbeat not allowed in state {:?}",
                    rt.state
                ))));
            }
            let hb = NodeHeartbeat {
                lease_id: rt.lease.lease_id.clone(),
                node_id: rt.node_id.clone(),
                instance_id: rt.instance_id.clone(),
                instance_epoch: rt.accepted_instance_epoch,
                accepted_contract_version: rt.accepted_contract_version.clone(),
                descriptor_checksum: g.identity.contract_checksum.clone(),
                capability_generation: rt.capability_generation,
                load: NodeLoad {
                    session_count: 0,
                    port_count: 0,
                    bandwidth_bps: 0,
                    worker_count: 0,
                    blocking_job_count: 0,
                    file_task_count: 0,
                    event_subscriber_count: 0,
                    cpu_permille: 0,
                    degraded_reasons: Vec::new(),
                    drain_state: rt.state,
                },
            };
            (hb, rt.accepted_instance_epoch, rt.lease.lease_id.clone())
        };

        hb.load = self.load.current_load(hb.load.drain_state).await?;
        let resp = self.registry.heartbeat(hb).await?;
        self.apply_heartbeat_response(&expected_lease, expected_epoch, &resp)
            .await?;
        Ok(resp)
    }

    /// Apply a heartbeat response that has already been received (for tests and
    /// hosts that drive the registry client themselves).
    pub async fn apply_heartbeat_response(
        &self,
        expected_lease_id: &str,
        expected_epoch: cheetah_media_api::ids::MediaNodeInstanceEpoch,
        resp: &NodeHeartbeatResponse,
    ) -> Result<(), ControlPlaneError> {
        let mut replaced = false;
        {
            let mut g = self.inner.lock();
            let Some(rt) = g.runtime.as_mut() else {
                return Ok(());
            };
            if rt.lease.lease_id != expected_lease_id
                || rt.accepted_instance_epoch != expected_epoch
            {
                return Ok(());
            }
            if let Some(lease) = &resp.lease {
                if lease.accepted_instance_epoch != expected_epoch {
                    replaced = true;
                } else {
                    rt.lease = lease.clone();
                }
            }
            if resp.next_heartbeat_interval_ms > 0 {
                g.next_heartbeat_interval_ms = resp.next_heartbeat_interval_ms;
            }
        }
        if replaced {
            let (node_id, instance_id) = {
                let g = self.inner.lock();
                let rt = g.runtime.as_ref().unwrap();
                (rt.node_id.clone(), rt.instance_id.clone())
            };
            let _ = self
                .isolate(NodeIsolateRequest {
                    node_id,
                    instance_id,
                    reason: LeaseLossReason::InstanceReplaced,
                    force: true,
                })
                .await;
        }
        Ok(())
    }

    /// Check lease deadline and isolate if expired. Call from the host tick loop.
    pub async fn check_lease(&self) -> Result<(), ControlPlaneError> {
        let now = self.clock.now_ms();
        let action = {
            let g = self.inner.lock();
            let Some(rt) = g.runtime.as_ref() else {
                return Ok(());
            };
            if !matches!(rt.state, NodeState::Active | NodeState::Draining) {
                return Ok(());
            }
            if rt.lease.status == LeaseStatus::Revoked {
                Some(LeaseLossReason::LeaseRevoked)
            } else if now >= rt.lease.deadline_ms {
                Some(LeaseLossReason::LeaseExpired)
            } else {
                None
            }
        };
        if let Some(reason) = action {
            let (node_id, instance_id) = {
                let g = self.inner.lock();
                let rt = g.runtime.as_ref().unwrap();
                (rt.node_id.clone(), rt.instance_id.clone())
            };
            let _ = self
                .isolate(NodeIsolateRequest {
                    node_id,
                    instance_id,
                    reason,
                    force: true,
                })
                .await?;
        }
        Ok(())
    }

    /// Enter or leave drain mode.
    pub async fn drain(
        &self,
        request: NodeDrainRequest,
    ) -> Result<NodeDrainResponse, ControlPlaneError> {
        {
            let mut g = self.inner.lock();
            let Some(rt) = g.runtime.as_mut() else {
                return Err(ControlPlaneError::Media(MediaError::unavailable(
                    "node is not registered".to_string(),
                )));
            };
            if matches!(rt.state, NodeState::Stopped | NodeState::Deregistering) {
                return Err(ControlPlaneError::Conflict(
                    "node is shutting down".to_string(),
                ));
            }
            rt.state = NodeState::Draining;
            g.drain_deadline_ms = Some(request.drain_deadline_ms);
        }
        self.capacity.set_node_gate(false).await?;
        Ok(NodeDrainResponse {
            accepted: true,
            drain_deadline_ms: request.drain_deadline_ms,
        })
    }

    /// Leave drain and return to Active when the lease is still valid.
    pub async fn leave_drain(&self) -> Result<(), ControlPlaneError> {
        let open_gate = {
            let mut g = self.inner.lock();
            let Some(rt) = g.runtime.as_mut() else {
                return Err(ControlPlaneError::Media(MediaError::unavailable(
                    "node is not registered".to_string(),
                )));
            };
            if rt.state != NodeState::Draining {
                return Err(ControlPlaneError::Conflict(
                    "node is not draining".to_string(),
                ));
            }
            if rt.lease.status == LeaseStatus::Active && self.clock.now_ms() < rt.lease.deadline_ms
            {
                rt.state = NodeState::Active;
                g.drain_deadline_ms = None;
                true
            } else {
                rt.state = NodeState::Isolated;
                g.drain_deadline_ms = None;
                false
            }
        };
        self.capacity.set_node_gate(open_gate).await?;
        Ok(())
    }

    /// Isolate the node after lease loss. Rejects creates; keeps reads.
    pub async fn isolate(
        &self,
        request: NodeIsolateRequest,
    ) -> Result<NodeIsolateResponse, ControlPlaneError> {
        {
            let mut g = self.inner.lock();
            let Some(rt) = g.runtime.as_mut() else {
                return Err(ControlPlaneError::Media(MediaError::unavailable(
                    "node is not registered".to_string(),
                )));
            };
            if rt.node_id != request.node_id || rt.instance_id != request.instance_id {
                return Err(ControlPlaneError::NotFound(
                    "isolate target does not match this process".to_string(),
                ));
            }
            if matches!(rt.state, NodeState::Stopped) {
                return Ok(NodeIsolateResponse {
                    isolated: true,
                    state: NodeState::Stopped,
                });
            }
            rt.state = NodeState::Isolated;
            if request.force || request.reason != LeaseLossReason::RegistryUnreachable {
                rt.lease.status = match request.reason {
                    LeaseLossReason::LeaseRevoked => LeaseStatus::Revoked,
                    LeaseLossReason::LeaseExpired => LeaseStatus::Expired,
                    _ => rt.lease.status,
                };
            }
            let _ = request.reason;
        }
        self.capacity.set_node_gate(false).await?;
        Ok(NodeIsolateResponse {
            isolated: true,
            state: NodeState::Isolated,
        })
    }

    /// Drain then deregister. Deregister failure does not block returning after attempt.
    pub async fn shutdown(&self, reason: &str) -> Result<(), ControlPlaneError> {
        let now = self.clock.now_ms();
        let _ = self
            .drain(NodeDrainRequest {
                drain_deadline_ms: now.saturating_add(30_000),
                reason: reason.to_string(),
                force: true,
            })
            .await;

        let (node_id, instance_id) = {
            let mut g = self.inner.lock();
            if let Some(rt) = g.runtime.as_mut() {
                rt.state = NodeState::Deregistering;
                (rt.node_id.clone(), rt.instance_id.clone())
            } else {
                return Ok(());
            }
        };

        let dereg = self
            .registry
            .deregister(NodeDeregisterRequest {
                node_id,
                instance_id,
                reason: reason.to_string(),
            })
            .await;
        // Always transition to Stopped and close the gate, even if deregister failed.
        {
            let mut g = self.inner.lock();
            if let Some(rt) = g.runtime.as_mut() {
                rt.state = NodeState::Stopped;
            }
        }
        let _ = self.capacity.set_node_gate(false).await;
        dereg.map(|_| ())
    }

    /// Current register backoff in milliseconds (for host retry scheduling).
    pub fn register_backoff_ms(&self) -> u64 {
        self.inner.lock().register_backoff_ms
    }
}

fn placeholder_runtime(identity: &NodeIdentity, state: NodeState) -> NodeRuntimeState {
    NodeRuntimeState {
        node_id: identity.node_id.clone(),
        instance_id: identity.instance_id.clone(),
        accepted_instance_epoch: identity.instance_epoch,
        state,
        lease: MediaNodeLease {
            lease_id: String::new(),
            status: LeaseStatus::Pending,
            deadline_ms: 0,
            heartbeat_interval_ms: 5_000,
            cluster_time_ms: 0,
            accepted_contract_version: String::new(),
            accepted_instance_epoch: identity.instance_epoch,
        },
        accepted_contract_version: String::new(),
        control_endpoint: identity.control_endpoint.clone(),
        network_zone: identity.network_zone.clone(),
        region: identity.region.clone(),
        labels: identity.labels.clone(),
        advertised_media_addresses: identity.advertised_media_addresses.clone(),
        build_version: identity.build_version.clone(),
        capability_generation: identity.capability_generation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_media_api::capacity::CapacityLimits;
    use cheetah_media_api::ids::{MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;

    struct FakeRegistry {
        register_calls: AtomicUsize,
        fail_register: Mutex<bool>,
        fail_deregister: Mutex<bool>,
        epoch: Mutex<u64>,
        lease_ttl_ms: i64,
        heartbeat_interval_ms: u64,
    }

    impl FakeRegistry {
        fn new() -> Self {
            Self {
                register_calls: AtomicUsize::new(0),
                fail_register: Mutex::new(false),
                fail_deregister: Mutex::new(false),
                epoch: Mutex::new(7),
                lease_ttl_ms: 10_000,
                heartbeat_interval_ms: 2_000,
            }
        }
    }

    #[async_trait]
    impl RegistryClient for FakeRegistry {
        async fn register(
            &self,
            _request: NodeRegistrationRequest,
        ) -> Result<NodeRegistrationResponse, ControlPlaneError> {
            self.register_calls.fetch_add(1, Ordering::SeqCst);
            if *self.fail_register.lock() {
                return Err(ControlPlaneError::Media(MediaError::unavailable(
                    "registry down".to_string(),
                )));
            }
            let epoch = {
                let mut e = self.epoch.lock();
                let v = *e;
                *e += 1;
                MediaNodeInstanceEpoch(v)
            };
            // Fixed base time so FakeClock tests remain deterministic.
            let now = 1_000_000i64;
            Ok(NodeRegistrationResponse {
                instance_epoch: epoch,
                lease: MediaNodeLease {
                    lease_id: format!("lease-{}", epoch.0),
                    status: LeaseStatus::Active,
                    deadline_ms: now + self.lease_ttl_ms,
                    heartbeat_interval_ms: self.heartbeat_interval_ms,
                    cluster_time_ms: now,
                    accepted_contract_version: "v1".to_string(),
                    accepted_instance_epoch: epoch,
                },
                accepted_contract_version: "v1".to_string(),
                cluster_time_ms: now,
            })
        }

        async fn heartbeat(
            &self,
            heartbeat: NodeHeartbeat,
        ) -> Result<NodeHeartbeatResponse, ControlPlaneError> {
            Ok(NodeHeartbeatResponse {
                lease: Some(MediaNodeLease {
                    lease_id: heartbeat.lease_id,
                    status: LeaseStatus::Active,
                    deadline_ms: 1_000_000 + self.lease_ttl_ms,
                    heartbeat_interval_ms: self.heartbeat_interval_ms,
                    cluster_time_ms: 1_000_000,
                    accepted_contract_version: heartbeat.accepted_contract_version,
                    accepted_instance_epoch: heartbeat.instance_epoch,
                }),
                next_heartbeat_interval_ms: self.heartbeat_interval_ms,
            })
        }

        async fn deregister(
            &self,
            _request: NodeDeregisterRequest,
        ) -> Result<NodeDeregisterResponse, ControlPlaneError> {
            if *self.fail_deregister.lock() {
                return Err(ControlPlaneError::Media(MediaError::unavailable(
                    "deregister failed".to_string(),
                )));
            }
            Ok(NodeDeregisterResponse { acknowledged: true })
        }
    }

    fn identity() -> NodeIdentity {
        NodeIdentity {
            node_id: MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            instance_id: MediaNodeInstanceId::new("550e8401-e29b-41d4-a716-446655440001").unwrap(),
            instance_epoch: MediaNodeInstanceEpoch(0),
            control_endpoint: "https://node:50051".to_string(),
            network_zone: Some("zone-a".to_string()),
            region: None,
            labels: HashMap::new(),
            advertised_media_addresses: vec!["rtp://node:10000".to_string()],
            build_version: "0.1.0".to_string(),
            contract_range: ">=1.0.0,<2.0.0".to_string(),
            contract_checksum: "sha256:abc".to_string(),
            capability_generation: 1,
        }
    }

    fn supervisor(registry: Arc<FakeRegistry>, clock: Arc<FakeClock>) -> NodeSupervisor {
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
        NodeSupervisor::new(identity(), capacity, registry, load, clock)
    }

    #[tokio::test]
    async fn register_opens_create_gate_on_success() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        let sup = supervisor(registry, clock);
        assert!(!sup.mutations_allowed());
        sup.register().await.unwrap();
        assert_eq!(sup.state(), NodeState::Active);
        assert!(sup.mutations_allowed());
        assert!(sup.capacity.snapshot().await.unwrap().node_gate_open);
    }

    #[tokio::test]
    async fn register_failure_keeps_gate_closed() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        *registry.fail_register.lock() = true;
        let sup = supervisor(registry, clock);
        assert!(sup.register().await.is_err());
        assert_eq!(sup.state(), NodeState::Registering);
        assert!(!sup.capacity.snapshot().await.unwrap().node_gate_open);
        assert!(sup.register_backoff_ms() >= 2_000);
    }

    #[tokio::test]
    async fn lease_expiry_isolates_and_rejects_creates() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        let sup = supervisor(registry, clock.clone());
        sup.register().await.unwrap();
        assert!(sup.mutations_allowed());

        clock.advance(20_000);
        sup.check_lease().await.unwrap();
        assert_eq!(sup.state(), NodeState::Isolated);
        assert!(!sup.mutations_allowed());
        assert!(sup.reads_allowed());
        assert!(!sup.capacity.snapshot().await.unwrap().node_gate_open);
    }

    #[tokio::test]
    async fn drain_rejects_mutations() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        let sup = supervisor(registry, clock);
        sup.register().await.unwrap();
        let resp = sup
            .drain(NodeDrainRequest {
                drain_deadline_ms: 1_000_000 + 5_000,
                reason: "roll".to_string(),
                force: false,
            })
            .await
            .unwrap();
        assert!(resp.accepted);
        assert_eq!(sup.state(), NodeState::Draining);
        assert!(!sup.mutations_allowed());
        assert!(sup.reads_allowed());
    }

    #[tokio::test]
    async fn leave_drain_restores_active_with_valid_lease() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        let sup = supervisor(registry, clock);
        sup.register().await.unwrap();
        sup.drain(NodeDrainRequest {
            drain_deadline_ms: 1_000_000 + 5_000,
            reason: "tmp".to_string(),
            force: false,
        })
        .await
        .unwrap();
        sup.leave_drain().await.unwrap();
        assert_eq!(sup.state(), NodeState::Active);
        assert!(sup.mutations_allowed());
    }

    #[tokio::test]
    async fn shutdown_stops_even_if_deregister_fails() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        *registry.fail_deregister.lock() = true;
        let sup = supervisor(registry, clock);
        sup.register().await.unwrap();
        let err = sup.shutdown("bye").await;
        assert!(err.is_err());
        assert_eq!(sup.state(), NodeState::Stopped);
        assert!(!sup.capacity.snapshot().await.unwrap().node_gate_open);
    }

    #[tokio::test]
    async fn stale_heartbeat_epoch_is_ignored_via_apply() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        let sup = supervisor(registry, clock);
        let reg = sup.register().await.unwrap();
        let lease_id = reg.lease.lease_id.clone();
        let epoch = reg.instance_epoch;

        // Stale response with wrong epoch must not change the current lease id.
        let stale = NodeHeartbeatResponse {
            lease: Some(MediaNodeLease {
                lease_id: "other".to_string(),
                status: LeaseStatus::Active,
                deadline_ms: 9_999_999,
                heartbeat_interval_ms: 100,
                cluster_time_ms: 0,
                accepted_contract_version: "v1".to_string(),
                accepted_instance_epoch: MediaNodeInstanceEpoch(epoch.0 + 99),
            }),
            next_heartbeat_interval_ms: 100,
        };
        // Wrong expected epoch => no-op
        sup.apply_heartbeat_response(&lease_id, MediaNodeInstanceEpoch(epoch.0 + 1), &stale)
            .await
            .unwrap();
        assert_eq!(sup.runtime_state().unwrap().lease.lease_id, lease_id);

        // Matching epoch but lease reports replacement => isolate
        sup.apply_heartbeat_response(&lease_id, epoch, &stale)
            .await
            .unwrap();
        assert_eq!(sup.state(), NodeState::Isolated);
    }

    #[tokio::test]
    async fn heartbeat_succeeds_while_draining() {
        let clock = Arc::new(FakeClock::new(1_000_000));
        let registry = Arc::new(FakeRegistry::new());
        let sup = supervisor(registry, clock);
        sup.register().await.unwrap();
        sup.drain(NodeDrainRequest {
            drain_deadline_ms: 1_000_000 + 5_000,
            reason: "roll".to_string(),
            force: false,
        })
        .await
        .unwrap();
        let resp = sup.heartbeat().await.unwrap();
        assert!(resp.next_heartbeat_interval_ms > 0);
        assert_eq!(sup.state(), NodeState::Draining);
    }
}
