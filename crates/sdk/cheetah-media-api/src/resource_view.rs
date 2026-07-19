//! Controlled-resource view returned by control-plane Get/List queries.
//!
//! 控制面 Get/List 查询返回的受控资源视图。

use serde::{Deserialize, Serialize};

use crate::error::MediaError;
use crate::fencing::ControlledResourceRef;
use crate::ids::{
    MediaBindingId, MediaKey, MediaNodeInstanceEpoch, MediaSessionId, OwnerEpoch,
    ResourceGeneration, TenantId,
};

/// Stable metadata for a controlled resource.
///
/// Fields suffixed with `registered_` are snapshots taken when the resource was
/// first recorded; the corresponding top-level fields on `ControlledResourceView`
/// hold the current values and may advance during takeover or migration.
///
/// 受控资源的稳定元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlledResourceMeta {
    pub tenant_id: TenantId,
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    pub resource_kind: String,
    pub resource_handle: String,
    pub registered_owner_epoch: OwnerEpoch,
    pub registered_node_instance_epoch: MediaNodeInstanceEpoch,
    pub registered_generation: ResourceGeneration,
}

impl ControlledResourceMeta {
    /// Build metadata from a controlled resource reference and the generation
    /// recorded at resource creation.
    pub fn from_ref(resource: &ControlledResourceRef, generation: ResourceGeneration) -> Self {
        Self {
            tenant_id: resource.tenant_id.clone(),
            media_session_id: resource.media_session_id.clone(),
            media_binding_id: resource.media_binding_id.clone(),
            resource_kind: resource.resource_kind.clone(),
            resource_handle: resource.resource_handle.clone(),
            registered_owner_epoch: resource.owner_epoch,
            registered_node_instance_epoch: resource.node_instance_epoch,
            registered_generation: generation,
        }
    }
}

/// Full view of a controlled resource, parameterized by the provider-specific
/// state type.
///
/// `meta` holds the immutable identifiers and the registration-time snapshot of
/// owner/epoch/generation. The top-level `generation`, `node_instance_epoch`,
/// and `accepted_owner_epoch` fields are the current fencing values, which may
/// differ after a takeover or reconciliation.
///
/// 受控资源的完整视图，以 provider 特定的状态类型为参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlledResourceView<S> {
    pub meta: ControlledResourceMeta,
    pub state: S,
    /// Current generation after fencing/reconciliation.
    pub generation: ResourceGeneration,
    /// Current node instance epoch after fencing/reconciliation.
    pub node_instance_epoch: MediaNodeInstanceEpoch,
    /// Current accepted owner epoch after fencing/reconciliation.
    pub accepted_owner_epoch: OwnerEpoch,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub terminal_ms: Option<i64>,
    /// Last safe error recorded for the resource, if any.
    pub last_error: Option<MediaError>,
    /// Media key for the resource when it is addressable as a stream.
    pub media_key: Option<MediaKey>,
    /// Opaque output reference (e.g. a file handle) when the resource produces
    /// a retrievable artifact. URLs are generated separately by a resolver.
    pub output_ref: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource_ref() -> ControlledResourceRef {
        ControlledResourceRef {
            tenant_id: TenantId::new("tenant-1").unwrap(),
            media_session_id: None,
            media_binding_id: None,
            resource_kind: "session".to_string(),
            resource_handle: "h1".to_string(),
            owner_epoch: OwnerEpoch(7),
            node_instance_epoch: MediaNodeInstanceEpoch(42),
            generation: ResourceGeneration(3),
        }
    }

    #[test]
    fn meta_round_trips_from_resource_ref() {
        let meta = ControlledResourceMeta::from_ref(&resource_ref(), ResourceGeneration(4));
        let json = serde_json::to_string(&meta).unwrap();
        let decoded: ControlledResourceMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, decoded);
    }

    #[test]
    fn view_round_trips_with_typed_state() {
        let meta = ControlledResourceMeta::from_ref(&resource_ref(), ResourceGeneration(4));
        let view = ControlledResourceView {
            meta,
            state: "running".to_string(),
            generation: ResourceGeneration(4),
            node_instance_epoch: MediaNodeInstanceEpoch(42),
            accepted_owner_epoch: OwnerEpoch(7),
            created_ms: 1000,
            updated_ms: 2000,
            terminal_ms: None,
            last_error: None,
            media_key: None,
            output_ref: Some("file-handle-1".to_string()),
        };
        let json = serde_json::to_string(&view).unwrap();
        let decoded: ControlledResourceView<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(view, decoded);
    }

    #[test]
    fn current_values_can_differ_from_registered() {
        let meta = ControlledResourceMeta::from_ref(&resource_ref(), ResourceGeneration(4));
        let view = ControlledResourceView {
            meta,
            state: "running".to_string(),
            generation: ResourceGeneration(6),
            node_instance_epoch: MediaNodeInstanceEpoch(43),
            accepted_owner_epoch: OwnerEpoch(9),
            created_ms: 1000,
            updated_ms: 2000,
            terminal_ms: None,
            last_error: None,
            media_key: None,
            output_ref: None,
        };
        assert_eq!(view.meta.registered_generation, ResourceGeneration(4));
        assert_eq!(view.meta.registered_owner_epoch, OwnerEpoch(7));
        assert_eq!(
            view.meta.registered_node_instance_epoch,
            MediaNodeInstanceEpoch(42)
        );
        assert_eq!(view.generation, ResourceGeneration(6));
        assert_eq!(view.accepted_owner_epoch, OwnerEpoch(9));
        assert_eq!(view.node_instance_epoch, MediaNodeInstanceEpoch(43));
    }
}
