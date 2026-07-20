//! Capacity request/snapshot/permit types.
//!
//! 容量请求、快照与许可类型。

use serde::{Deserialize, Serialize};

/// A vector of counted resource dimensions used for capacity requests,
/// limits and snapshots.
///
/// 容量请求、上限与快照使用的资源维度向量。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CapacityVector {
    pub session_count: u64,
    pub port_count: u64,
    pub bandwidth_bps: u64,
    pub worker_count: u64,
    pub blocking_job_count: u64,
    pub file_task_count: u64,
    pub event_subscriber_count: u64,
    pub cpu_permille: u64,
}

/// Request to acquire capacity from the `MediaCapacityApi`.
///
/// 向 `MediaCapacityApi` 申请容量的请求。
pub type CapacityRequest = CapacityVector;

/// Hard limits for each resource dimension.
///
/// 每个资源维度的硬上限。
pub type CapacityLimits = CapacityVector;

/// Snapshot of current capacity usage and availability.
///
/// 当前容量使用与可用性的快照。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CapacitySnapshot {
    pub used: CapacityVector,
    pub remaining: CapacityVector,
    /// If false, the node gate is closed and new resource creation is rejected.
    pub node_gate_open: bool,
    /// Cluster time at which the snapshot was taken.
    pub updated_at_ms: i64,
}

/// Opaque permit returned by `MediaCapacityApi::acquire`.
///
/// Implementations are expected to release their reservation when the permit
/// is dropped.
///
/// `MediaCapacityApi::acquire` 返回的不透明许可。许可在 drop 时释放其预留。
pub trait CapacityPermit: Send + std::fmt::Debug {
    /// Optional resource handle this permit is associated with.
    fn resource_handle(&self) -> Option<&str>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_vector_round_trips() {
        let v = CapacityVector {
            session_count: 1,
            port_count: 2,
            bandwidth_bps: 3,
            worker_count: 4,
            blocking_job_count: 5,
            file_task_count: 6,
            event_subscriber_count: 7,
            cpu_permille: 100,
        };
        let json = serde_json::to_string(&v).unwrap();
        let decoded: CapacityVector = serde_json::from_str(&json).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn capacity_snapshot_round_trips() {
        let snap = CapacitySnapshot {
            used: CapacityVector {
                session_count: 1,
                port_count: 0,
                bandwidth_bps: 0,
                worker_count: 0,
                blocking_job_count: 0,
                file_task_count: 0,
                event_subscriber_count: 0,
                cpu_permille: 0,
            },
            remaining: CapacityVector {
                session_count: 99,
                port_count: 0,
                bandwidth_bps: 0,
                worker_count: 0,
                blocking_job_count: 0,
                file_task_count: 0,
                event_subscriber_count: 0,
                cpu_permille: 0,
            },
            node_gate_open: true,
            updated_at_ms: 1_000_000,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let decoded: CapacitySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, decoded);
    }
}
