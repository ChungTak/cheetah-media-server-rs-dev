//! Media-node identity and registration metadata.
//!
//! 媒体节点身份与注册元数据。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{MediaError, MediaErrorCode};
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
}
