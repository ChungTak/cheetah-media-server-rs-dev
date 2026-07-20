//! Media-node identity and registration metadata.
//!
//! 媒体节点身份与注册元数据。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{MediaError, MediaErrorCode};
use crate::fencing::{LeaseLossReason, MediaNodeLease, NodeState};
use crate::ids::{MediaNodeId, MediaNodeInstanceEpoch, MediaNodeInstanceId, OwnerEpoch};

/// Stable, deployment-level identity of a media node.
///
/// `node_id` is stable across restarts. `instance_id` and `instance_epoch`
/// identify a particular process incarnation and are assigned by the signaling
/// registry.
///
/// 媒体节点的稳定部署级身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub node_id: MediaNodeId,
    pub instance_id: MediaNodeInstanceId,
    pub instance_epoch: MediaNodeInstanceEpoch,
    /// gRPC control endpoint advertised to the signaling registry.
    pub control_endpoint: String,
    pub network_zone: Option<String>,
    pub region: Option<String>,
    pub labels: HashMap<String, String>,
    /// Advertised media addresses (e.g. `rtp://node:10000`) for stream routing.
    pub advertised_media_addresses: Vec<String>,
    pub build_version: String,
    /// Accepted contract version range, e.g. `>=1.0.0, <2.0.0`.
    pub contract_range: String,
    /// Checksum of the accepted contract descriptor.
    pub contract_checksum: String,
    /// Capability generation reported by this node.
    pub capability_generation: u64,
}

/// Request to register or re-register a media node with the signaling registry.
///
/// 向信号注册中心注册或重新注册媒体节点的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRegistrationRequest {
    pub node_identity: NodeIdentity,
    /// Previous lease ID, if re-registering within the lease validity window.
    pub previous_lease_id: Option<String>,
}

/// Response returned by the signaling registry for a successful node registration.
///
/// 信号注册中心成功注册节点后返回的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRegistrationResponse {
    /// Instance epoch assigned by the registry for this process incarnation.
    pub instance_epoch: MediaNodeInstanceEpoch,
    /// Lease granted by the registry.
    pub lease: MediaNodeLease,
    /// Contract version accepted by the registry.
    pub accepted_contract_version: String,
    /// Cluster time at lease issuance, as a UTC millisecond timestamp.
    pub cluster_time_ms: i64,
}

/// Resource usage and health reported by a node.
///
/// 节点上报的资源使用与健康状况。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLoad {
    pub session_count: u64,
    pub port_count: u64,
    pub bandwidth_bps: u64,
    pub worker_count: u64,
    pub blocking_job_count: u64,
    pub file_task_count: u64,
    pub event_subscriber_count: u64,
    /// Normalized CPU load as a permille value (0–1000).
    pub cpu_permille: u64,
    pub degraded_reasons: Vec<String>,
    /// Current drain state reported by the node.
    pub drain_state: NodeState,
}

/// Heartbeat sent from the media node to the signaling registry.
///
/// 媒体节点向信号注册中心发送的心跳。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHeartbeat {
    pub lease_id: String,
    pub node_id: MediaNodeId,
    pub instance_id: MediaNodeInstanceId,
    pub instance_epoch: MediaNodeInstanceEpoch,
    pub accepted_contract_version: String,
    /// Checksum of the descriptor accepted by the registry.
    pub descriptor_checksum: String,
    pub capability_generation: u64,
    pub load: NodeLoad,
}

/// Registry response to a node heartbeat.
///
/// 注册中心对心跳的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHeartbeatResponse {
    /// Updated lease, if the registry chose to extend it.
    pub lease: Option<MediaNodeLease>,
    /// Heartbeat interval requested by the registry in milliseconds.
    pub next_heartbeat_interval_ms: u64,
}

/// Request to put a node into drain and eventually deregister it.
///
/// 请求节点进入 drain 并最终注销。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDrainRequest {
    /// Deadline by which the node should finish active work, in milliseconds.
    pub drain_deadline_ms: i64,
    /// Human-readable reason for the drain.
    pub reason: String,
    /// If true, the node should stop accepting reads as well as creates.
    pub force: bool,
}

/// Response confirming or rejecting a drain request.
///
/// 确认或拒绝 drain 请求的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDrainResponse {
    pub accepted: bool,
    /// Effective deadline by which the node must finish draining.
    pub drain_deadline_ms: i64,
}

/// Request to deregister a node instance.
///
/// 注销节点实例的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDeregisterRequest {
    pub node_id: MediaNodeId,
    pub instance_id: MediaNodeInstanceId,
    pub reason: String,
}

/// Response confirming a deregister request.
///
/// 注销节点实例请求的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDeregisterResponse {
    pub acknowledged: bool,
}

/// Request to isolate a node after lease loss.
///
/// 租约丢失后隔离节点的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIsolateRequest {
    pub node_id: MediaNodeId,
    pub instance_id: MediaNodeInstanceId,
    pub reason: LeaseLossReason,
    /// If true, the node should isolate immediately without waiting for the
    /// lease deadline.
    pub force: bool,
}

/// Response to a node isolation request.
///
/// 节点隔离请求的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIsolateResponse {
    pub isolated: bool,
    pub state: NodeState,
}

impl NodeIdentity {
    /// Validate that the identity contains all required fields.
    pub fn validate(&self) -> Result<(), MediaError> {
        if self.control_endpoint.is_empty() {
            return Err(MediaError::new(
                MediaErrorCode::InvalidArgument,
                "control_endpoint is required",
            ));
        }
        if self.build_version.is_empty() {
            return Err(MediaError::new(
                MediaErrorCode::InvalidArgument,
                "build_version is required",
            ));
        }
        if self.contract_range.is_empty() {
            return Err(MediaError::new(
                MediaErrorCode::InvalidArgument,
                "contract_range is required",
            ));
        }
        if self.contract_checksum.is_empty() {
            return Err(MediaError::new(
                MediaErrorCode::InvalidArgument,
                "contract_checksum is required",
            ));
        }
        Ok(())
    }

    /// Return the owner epoch used by resources created on this node instance.
    pub fn owner_epoch(&self) -> OwnerEpoch {
        OwnerEpoch(self.instance_epoch.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> NodeIdentity {
        NodeIdentity {
            node_id: MediaNodeId::new("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            instance_id: MediaNodeInstanceId::new("550e8401-e29b-41d4-a716-446655440001").unwrap(),
            instance_epoch: MediaNodeInstanceEpoch(42),
            control_endpoint: "https://node.example:50051".to_string(),
            network_zone: Some("zone-a".to_string()),
            region: Some("us-east".to_string()),
            labels: HashMap::new(),
            advertised_media_addresses: vec!["rtp://node.example:10000".to_string()],
            build_version: "0.1.0".to_string(),
            contract_range: ">=1.0.0, <2.0.0".to_string(),
            contract_checksum: "sha256:abc123".to_string(),
            capability_generation: 1,
        }
    }

    #[test]
    fn identity_round_trips() {
        let id = identity();
        let json = serde_json::to_string(&id).unwrap();
        let decoded: NodeIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(id, decoded);
    }

    #[test]
    fn identity_validates_required_fields() {
        let mut id = identity();
        id.control_endpoint = "".to_string();
        assert!(id.validate().is_err());

        id = identity();
        id.build_version = "".to_string();
        assert!(id.validate().is_err());

        id = identity();
        id.contract_range = "".to_string();
        assert!(id.validate().is_err());

        id = identity();
        id.contract_checksum = "".to_string();
        assert!(id.validate().is_err());
    }

    #[test]
    fn owner_epoch_derives_from_instance_epoch() {
        let id = identity();
        assert_eq!(id.owner_epoch(), OwnerEpoch(42));
    }

    #[test]
    fn heartbeat_round_trips() {
        let hb = NodeHeartbeat {
            lease_id: "lease-1".to_string(),
            node_id: identity().node_id,
            instance_id: identity().instance_id,
            instance_epoch: MediaNodeInstanceEpoch(7),
            accepted_contract_version: "v1".to_string(),
            descriptor_checksum: "sha256:abc".to_string(),
            capability_generation: 1,
            load: NodeLoad {
                session_count: 10,
                port_count: 4,
                bandwidth_bps: 1_000_000,
                worker_count: 2,
                blocking_job_count: 0,
                file_task_count: 1,
                event_subscriber_count: 3,
                cpu_permille: 123,
                degraded_reasons: vec!["disk slow".to_string()],
                drain_state: NodeState::Active,
            },
        };
        let json = serde_json::to_string(&hb).unwrap();
        let decoded: NodeHeartbeat = serde_json::from_str(&json).unwrap();
        assert_eq!(hb, decoded);

        let resp = NodeHeartbeatResponse {
            lease: None,
            next_heartbeat_interval_ms: 5_000,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: NodeHeartbeatResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn drain_and_deregister_round_trip() {
        let drain = NodeDrainRequest {
            drain_deadline_ms: 1_000_000,
            reason: "rolling restart".to_string(),
            force: false,
        };
        let json = serde_json::to_string(&drain).unwrap();
        let decoded: NodeDrainRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(drain, decoded);

        let resp = NodeDrainResponse {
            accepted: true,
            drain_deadline_ms: 1_000_000,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: NodeDrainResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);

        let deregister = NodeDeregisterRequest {
            node_id: identity().node_id,
            instance_id: identity().instance_id,
            reason: "shutdown".to_string(),
        };
        let json = serde_json::to_string(&deregister).unwrap();
        let decoded: NodeDeregisterRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deregister, decoded);

        let deregister_resp = NodeDeregisterResponse { acknowledged: true };
        let json = serde_json::to_string(&deregister_resp).unwrap();
        let decoded: NodeDeregisterResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deregister_resp, decoded);
    }

    #[test]
    fn isolate_request_and_response_round_trip() {
        let req = NodeIsolateRequest {
            node_id: identity().node_id,
            instance_id: identity().instance_id,
            reason: LeaseLossReason::RegistryUnreachable,
            force: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: NodeIsolateRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);

        let resp = NodeIsolateResponse {
            isolated: true,
            state: NodeState::Isolated,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: NodeIsolateResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn registration_request_and_response_round_trip() {
        let req = NodeRegistrationRequest {
            node_identity: identity(),
            previous_lease_id: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: NodeRegistrationRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);

        let resp = NodeRegistrationResponse {
            instance_epoch: MediaNodeInstanceEpoch(7),
            lease: MediaNodeLease {
                lease_id: "lease-1".to_string(),
                status: crate::fencing::LeaseStatus::Active,
                deadline_ms: 1_000_000,
                heartbeat_interval_ms: 5_000,
                cluster_time_ms: 0,
                accepted_contract_version: "v1".to_string(),
                accepted_instance_epoch: MediaNodeInstanceEpoch(7),
            },
            accepted_contract_version: "v1".to_string(),
            cluster_time_ms: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: NodeRegistrationResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }
}
