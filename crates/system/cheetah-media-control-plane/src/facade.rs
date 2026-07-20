//! Control-plane facade and runtime-neutral API.
//!
//! 控制面 facade 与运行时无关 API。

use std::sync::Arc;

use cheetah_runtime_api::RuntimeApi;

use crate::event_store::EventStore;
use crate::reconciler::{OrphanReconciler, Reconciler};
use crate::store::{IdempotencyStore, OrphanStore, ResourceStore};

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
        }
    }
}
