//! Fencing and controlled-resource metadata for the signaling control plane.
//!
//! 控制面 fencing 与受控资源元数据。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ids::{
    MediaBindingId, MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId, MediaSessionId,
    OwnerEpoch, ResourceGeneration, TenantId,
};

/// Origin of a controlled resource.
///
/// 受控资源的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOrigin {
    /// Created by the cluster signaling control plane.
    #[default]
    Cluster,
    /// Created by a local media adapter (Native/ZLM) for local-only lifecycle.
    Local,
}

/// Reference to a controlled resource used in errors, audit logs, and cleanup.
///
/// 错误、审计和清理中使用的受控资源引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlledResourceRef {
    pub tenant_id: TenantId,
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    pub resource_kind: String,
    pub resource_handle: String,
    pub owner_epoch: OwnerEpoch,
    pub node_instance_epoch: MediaNodeInstanceEpoch,
    pub generation: ResourceGeneration,
    #[serde(default)]
    pub origin: ResourceOrigin,
}

impl ControlledResourceRef {
    /// Return a short, safe display string that does not include tenant or
    /// session identifiers.
    pub fn safe_display(&self) -> String {
        format!("{}:{}", self.resource_kind, self.resource_handle)
    }
}

/// Lifecycle state of a media node instance from the control-plane view.
///
/// 控制面视角的媒体节点实例生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    #[default]
    Disabled,
    Binding,
    Registering,
    Active,
    Draining,
    Isolated,
    Deregistering,
    Stopped,
}

/// Status of a node's registration lease.
///
/// 节点注册租约的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    #[default]
    Pending,
    Active,
    Expired,
    Revoked,
}

/// Reason a node lease was lost, triggering isolation.
///
/// 节点租约丢失并触发隔离的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseLossReason {
    RegistryUnreachable,
    LeaseExpired,
    LeaseRevoked,
    ContractVersionRejected,
    InstanceReplaced,
    AdminInitiated,
}

/// A registration lease granted by the signaling registry.
///
/// 由信号注册中心授予的注册租约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaNodeLease {
    pub lease_id: String,
    pub status: LeaseStatus,
    /// Lease deadline as a UTC millisecond timestamp.
    pub deadline_ms: i64,
    /// Heartbeat interval requested by the registry in milliseconds.
    pub heartbeat_interval_ms: u64,
    /// Cluster time at lease issuance, as a UTC millisecond timestamp.
    pub cluster_time_ms: i64,
    /// Contract version accepted by the registry.
    pub accepted_contract_version: String,
    /// Instance epoch accepted by the registry.
    pub accepted_instance_epoch: MediaNodeInstanceEpoch,
}

/// Runtime state held by the node supervisor for fencing decisions.
///
/// 节点 supervisor 为 fencing 决策保存的运行时状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRuntimeState {
    pub node_id: MediaNodeId,
    pub instance_id: MediaNodeInstanceId,
    pub accepted_instance_epoch: MediaNodeInstanceEpoch,
    pub state: NodeState,
    pub lease: MediaNodeLease,
    pub accepted_contract_version: String,
    pub control_endpoint: String,
    pub network_zone: Option<String>,
    pub region: Option<String>,
    pub labels: HashMap<String, String>,
    pub advertised_media_addresses: Vec<String>,
    pub build_version: String,
    pub capability_generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controlled_resource_ref_round_trips() {
        let tenant = TenantId::new("tenant-1").unwrap();
        let ref_ = ControlledResourceRef {
            tenant_id: tenant,
            media_session_id: None,
            media_binding_id: None,
            resource_kind: "session".to_string(),
            resource_handle: "handle-1".to_string(),
            owner_epoch: OwnerEpoch(7),
            node_instance_epoch: MediaNodeInstanceEpoch(42),
            generation: ResourceGeneration(3),
            origin: ResourceOrigin::Cluster,
        };
        let json = serde_json::to_string(&ref_).unwrap();
        let decoded: ControlledResourceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(ref_, decoded);
        assert_eq!(ref_.safe_display(), "session:handle-1");
    }

    #[test]
    fn node_state_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&NodeState::Deregistering).unwrap(),
            "\"deregistering\""
        );
    }

    #[test]
    fn node_runtime_state_round_trips() {
        let node_id = MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let instance_id = MediaNodeInstanceId::new("550e8401-e29b-41d4-a716-446655440000").unwrap();
        let state = NodeRuntimeState {
            node_id,
            instance_id,
            accepted_instance_epoch: MediaNodeInstanceEpoch(7),
            state: NodeState::Active,
            lease: MediaNodeLease {
                lease_id: "lease-1".to_string(),
                status: LeaseStatus::Active,
                deadline_ms: 1_000_000,
                heartbeat_interval_ms: 5_000,
                cluster_time_ms: 0,
                accepted_contract_version: "v1".to_string(),
                accepted_instance_epoch: MediaNodeInstanceEpoch(7),
            },
            accepted_contract_version: "v1".to_string(),
            control_endpoint: "https://node.example:50051".to_string(),
            network_zone: Some("zone-a".to_string()),
            region: Some("us-east".to_string()),
            labels: HashMap::new(),
            advertised_media_addresses: vec!["rtp://node.example:10000".to_string()],
            build_version: "0.1.0".to_string(),
            capability_generation: 1,
        };
        let json = serde_json::to_string(&state).unwrap();
        let decoded: NodeRuntimeState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, decoded);
    }
}
