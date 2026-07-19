//! Fencing and controlled-resource metadata for the signaling control plane.
//!
//! 控制面 fencing 与受控资源元数据。

use serde::{Deserialize, Serialize};

use crate::ids::{
    MediaBindingId, MediaNodeInstanceEpoch, MediaSessionId, OwnerEpoch, ResourceGeneration,
    TenantId,
};

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
}

impl ControlledResourceRef {
    /// Return a short, safe display string that does not include tenant or
    /// session identifiers.
    pub fn safe_display(&self) -> String {
        format!("{}:{}", self.resource_kind, self.resource_handle)
    }
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
        };
        let json = serde_json::to_string(&ref_).unwrap();
        let decoded: ControlledResourceRef = serde_json::from_str(&json).unwrap();
        assert_eq!(ref_, decoded);
        assert_eq!(ref_.safe_display(), "session:handle-1");
    }
}
