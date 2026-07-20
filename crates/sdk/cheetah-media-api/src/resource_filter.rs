//! Unified resource filters for control-plane queries.
//!
//! 控制面查询的统一资源过滤条件。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{MediaError, MediaErrorCode, Result};
use crate::ids::{
    MediaBindingId, MediaKey, MediaNodeInstanceEpoch, MediaSessionId, OwnerEpoch, TenantId,
};

/// Common lifecycle states for controlled resources.
///
/// Provider-specific states should be mappable to these values before the
/// filter is evaluated.
///
/// 受控资源的通用生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    /// Resource has been accepted but not yet started.
    Pending,
    /// Resource is actively running.
    Active,
    /// Resource is in the process of stopping.
    Stopping,
    /// Resource has stopped cleanly and is terminal.
    Stopped,
    /// Resource failed and is terminal.
    Failed,
    /// Resource state cannot be determined and must be reconciled.
    Unknown,
}

impl ResourceState {
    /// Return true if the state is terminal (no further active work).
    pub fn is_terminal(&self) -> bool {
        matches!(self, ResourceState::Stopped | ResourceState::Failed)
    }
}

/// Unified filter for controlled-resource queries.
///
/// All fields are combined with AND semantics. Unknown or illegal combinations
/// are rejected by `validate`.
///
/// 受控资源查询的统一过滤条件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceFilter {
    pub tenant_id: TenantId,
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    pub resource_handle: Option<String>,
    pub media_key: Option<MediaKey>,
    pub idempotency_key: Option<String>,
    pub state: Option<ResourceState>,
    /// When true, only return resources in a non-terminal state.
    pub non_terminal: bool,
    pub owner_epoch: Option<OwnerEpoch>,
    pub node_instance_epoch: Option<MediaNodeInstanceEpoch>,
    pub updated_before_ms: Option<i64>,
    pub updated_after_ms: Option<i64>,
}

impl ResourceFilter {
    /// Validate the filter constraints without touching external state.
    pub fn validate(&self) -> Result<()> {
        if self.tenant_id.as_str().is_empty() {
            return Err(MediaError::invalid_argument("tenant_id is required"));
        }

        if let (Some(after), Some(before)) = (self.updated_after_ms, self.updated_before_ms) {
            if after >= before {
                return Err(MediaError::invalid_argument(
                    "updated_after_ms must be strictly less than updated_before_ms",
                ));
            }
        }

        if let Some(state) = self.state {
            if self.non_terminal && state.is_terminal() {
                return Err(MediaError::new(
                    MediaErrorCode::InvalidArgument,
                    "state filter conflicts with non_terminal: selected state is terminal",
                ));
            }
        }

        Ok(())
    }

    /// Compute a stable hex digest of the filter for cursor scoping.
    ///
    /// The digest covers `tenant_id` and all present filter dimensions so that a
    /// cursor issued for one filter cannot be reused under a different one.
    pub fn digest(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| String::new());
        let hash = Sha256::digest(json.as_bytes());
        Self::hex(&hash)
    }

    fn hex(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write;
            write!(s, "{:02x}", b).unwrap();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::new("tenant-1").unwrap()
    }

    fn filter() -> ResourceFilter {
        ResourceFilter {
            tenant_id: tenant(),
            media_session_id: None,
            media_binding_id: None,
            resource_handle: None,
            media_key: None,
            idempotency_key: None,
            state: None,
            non_terminal: false,
            owner_epoch: None,
            node_instance_epoch: None,
            updated_before_ms: None,
            updated_after_ms: None,
        }
    }

    #[test]
    fn filter_validates_required_tenant() {
        let mut f = filter();
        // An empty tenant can only arrive through deserialization because
        // `TenantId::new` rejects it at construction time.
        f.tenant_id = serde_json::from_str("\"\"").unwrap();
        assert!(f.validate().is_err());
    }

    #[test]
    fn filter_rejects_inverted_time_window() {
        let mut f = filter();
        f.updated_after_ms = Some(200);
        f.updated_before_ms = Some(100);
        assert!(f.validate().is_err());
    }

    #[test]
    fn filter_accepts_valid_time_window() {
        let mut f = filter();
        f.updated_after_ms = Some(100);
        f.updated_before_ms = Some(200);
        assert!(f.validate().is_ok());
    }

    #[test]
    fn filter_rejects_terminal_state_with_non_terminal() {
        let mut f = filter();
        f.state = Some(ResourceState::Stopped);
        f.non_terminal = true;
        assert!(f.validate().is_err());
    }

    #[test]
    fn filter_accepts_non_terminal_state_with_non_terminal() {
        let mut f = filter();
        f.state = Some(ResourceState::Active);
        f.non_terminal = true;
        assert!(f.validate().is_ok());
    }

    #[test]
    fn digest_is_stable_and_sensitive_to_tenant() {
        let a = filter().digest();
        let mut f = filter();
        f.tenant_id = TenantId::new("tenant-2").unwrap();
        let b = f.digest();
        assert_ne!(a, b);
    }

    #[test]
    fn state_serializes_to_snake_case() {
        let json = serde_json::to_string(&ResourceState::Failed).unwrap();
        assert_eq!(json, "\"failed\"");
    }
}
