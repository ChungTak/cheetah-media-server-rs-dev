//! Control-plane facade and runtime-neutral API.
//!
//! 控制面 facade 与运行时无关 API。

use std::sync::Arc;

use parking_lot::Mutex;

use async_trait::async_trait;
use cheetah_media_api::admin::TlsComponent;
use cheetah_media_api::error::MediaError;
use cheetah_media_api::fencing::NodeRuntimeState;
use cheetah_runtime_api::RuntimeApi;

use crate::capacity::CapacityOrchestrator;
use crate::event_store::EventStore;
use crate::node_supervisor::NodeSupervisor;
use crate::reconciler::{OrphanReconciler, Reconciler};
use crate::store::{IdempotencyStore, OrphanStore, ResourceStore, StoreMaintenance};

/// Optional hook used by admin TLS rotation (SEC-02).
///
/// 管理面 TLS 轮换钩子。
#[async_trait]
pub trait TlsRotator: Send + Sync {
    /// Rotate the named TLS/cursor component. Returns whether material was applied.
    async fn rotate(&self, component: TlsComponent) -> Result<bool, MediaError>;
}

/// The control-plane context shared by the gRPC adapter and internal modules.
///
/// It does not expose `rusqlite`, Tokio, or tonic types. Store implementations
/// and runtime access are held behind trait objects so the same facade can be
/// used in tests with in-memory stores.
///
/// 控制面上下文，gRPC adapter 和内部模块共享。
#[derive(Clone)]
pub struct ControlPlane {
    pub runtime: Arc<dyn RuntimeApi>,
    pub idempotency: Arc<dyn IdempotencyStore>,
    pub resources: Arc<dyn ResourceStore>,
    pub events: Arc<dyn EventStore>,
    pub orphan: Arc<dyn OrphanStore>,
    pub reconciler: Arc<dyn Reconciler>,
    /// Optional store maintenance (checkpoint/stats). Required for admin OPS.
    pub store_maintenance: Option<Arc<dyn StoreMaintenance>>,
    /// Current node runtime state used for fencing and admin drain.
    pub node: Arc<Mutex<Option<NodeRuntimeState>>>,
    /// Optional capacity orchestrator so admin drain can close the create gate.
    pub capacity: Option<Arc<CapacityOrchestrator>>,
    /// Optional TLS rotator for admin rotate_tls.
    pub tls_rotator: Option<Arc<dyn TlsRotator>>,
    /// Optional node supervisor for NODE lifecycle (register/drain/lease).
    pub node_supervisor: Option<Arc<NodeSupervisor>>,
}

impl ControlPlane {
    /// Create a new control plane from a runtime and store handles.
    pub fn new(
        runtime: Arc<dyn RuntimeApi>,
        idempotency: Arc<dyn IdempotencyStore>,
        resources: Arc<dyn ResourceStore>,
        events: Arc<dyn EventStore>,
        orphan: Arc<dyn OrphanStore>,
    ) -> Self {
        let reconciler: Arc<dyn Reconciler> =
            Arc::new(OrphanReconciler::new(resources.clone(), orphan.clone()));
        Self {
            runtime,
            idempotency,
            resources,
            events,
            orphan,
            reconciler,
            store_maintenance: None,
            node: Arc::new(Mutex::new(None)),
            capacity: None,
            tls_rotator: None,
            node_supervisor: None,
        }
    }

    /// Attach store maintenance for checkpoint and diagnostics.
    pub fn with_store_maintenance(mut self, store: Arc<dyn StoreMaintenance>) -> Self {
        self.store_maintenance = Some(store);
        self
    }

    /// Attach the capacity orchestrator used by drain/create gating.
    pub fn with_capacity(mut self, capacity: Arc<CapacityOrchestrator>) -> Self {
        self.capacity = Some(capacity);
        self
    }

    /// Attach a TLS rotator for admin rotate_tls.
    pub fn with_tls_rotator(mut self, rotator: Arc<dyn TlsRotator>) -> Self {
        self.tls_rotator = Some(rotator);
        self
    }

    /// Attach the node supervisor and mirror its runtime state into `node`.
    pub fn with_node_supervisor(mut self, supervisor: Arc<NodeSupervisor>) -> Self {
        if let Some(rt) = supervisor.runtime_state() {
            *self.node.lock() = Some(rt);
        }
        self.node_supervisor = Some(supervisor);
        self
    }

    /// Refresh the cached node runtime snapshot from the supervisor, if any.
    pub fn sync_node_from_supervisor(&self) {
        let Some(sup) = &self.node_supervisor else {
            return;
        };
        *self.node.lock() = sup.runtime_state();
    }

    /// Replace the cached node runtime state.
    pub fn set_node_runtime(&self, state: Option<NodeRuntimeState>) {
        *self.node.lock() = state;
    }

    /// Snapshot the cached node runtime state.
    pub fn node_runtime(&self) -> Option<NodeRuntimeState> {
        self.node.lock().clone()
    }
}
