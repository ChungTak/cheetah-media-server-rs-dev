//! Durable idempotency types and canonical request digest.
//!
//! 持久幂等类型与规范请求摘要。

use std::collections::BTreeMap;

use cheetah_media_api::fencing::ControlledResourceRef;
use cheetah_media_api::ids::{MediaBindingId, MediaSessionId, TenantId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A client-supplied idempotency key scoped to a tenant and operation kind.
///
/// 客户端提供的幂等键，按租户与操作类型隔离。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey {
    pub tenant_id: TenantId,
    pub operation_kind: String,
    pub key: String,
}

impl IdempotencyKey {
    /// Create a new idempotency key.
    pub fn new(
        tenant_id: TenantId,
        operation_kind: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id,
            operation_kind: operation_kind.into(),
            key: key.into(),
        }
    }

    /// Validate that the key is non-empty and free of control characters.
    pub fn validate(&self) -> Result<(), super::ControlPlaneError> {
        if self.key.is_empty() {
            return Err(cheetah_media_api::error::MediaError::new(
                cheetah_media_api::error::MediaErrorCode::InvalidArgument,
                "idempotency key must be non-empty",
            )
            .into());
        }
        if self.key.chars().any(|c| c.is_control()) {
            return Err(cheetah_media_api::error::MediaError::new(
                cheetah_media_api::error::MediaErrorCode::InvalidArgument,
                "idempotency key must not contain control characters",
            )
            .into());
        }
        if self.operation_kind.is_empty() {
            return Err(cheetah_media_api::error::MediaError::new(
                cheetah_media_api::error::MediaErrorCode::InvalidArgument,
                "operation kind must be non-empty",
            )
            .into());
        }
        Ok(())
    }
}

/// State of an idempotency record in the store.
///
/// 幂等记录在 store 中的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyState {
    Prepared,
    Completed,
    Failed,
    Unknown,
}

/// A stable SHA-256 digest used to detect request canonicalization conflicts.
///
/// 用于检测请求规范化冲突的稳定 SHA-256 摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CanonicalDigest(pub [u8; 32]);

impl CanonicalDigest {
    /// Return the digest as a hex-encoded string.
    pub fn to_hex(&self) -> String {
        self.0.map(|b| format!("{b:02x}")).concat()
    }
}

/// Canonical request representation used for idempotent digest computation.
///
/// The digest covers the schema version, tenant, operation kind, target resource
/// reference, session/binding identifiers, and business parameters. It excludes
/// request/message/correlation IDs, tracing context, deadline, and retry
/// attempt count.
///
/// 用于幂等摘要计算的规范化请求表示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalRequest {
    pub schema_version: u32,
    pub tenant_id: TenantId,
    pub operation_kind: String,
    pub target: Option<ControlledResourceRef>,
    pub media_session_id: Option<MediaSessionId>,
    pub media_binding_id: Option<MediaBindingId>,
    /// Normalized business parameters as a JSON value. Object keys are sorted
    /// before digesting so equivalent payloads produce the same digest.
    pub business_params: Value,
}

impl CanonicalRequest {
    /// Validate and compute the canonical SHA-256 digest of this request.
    pub fn digest(&self) -> Result<CanonicalDigest, super::ControlPlaneError> {
        let canonical = canonicalize_value(&serde_json::to_value(self).map_err(|e| {
            super::ControlPlaneError::Serialization(format!("failed to serialize request: {e}"))
        })?);
        let json = serde_json::to_string(&canonical).map_err(|e| {
            super::ControlPlaneError::Serialization(format!("failed to serialize request: {e}"))
        })?;
        let hash = Sha256::digest(json);
        Ok(CanonicalDigest(hash.into()))
    }
}

/// Recursively sort JSON object keys so equivalent objects produce identical
/// canonical JSON.
fn canonicalize_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            // Explicitly sort keys into a BTreeMap so digest stability does not
            // depend on serde_json's default map backing type.
            let sorted: BTreeMap<_, _> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize_value(v)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_value).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use cheetah_media_api::ids::{OwnerEpoch, ResourceGeneration};

    use super::*;

    fn request() -> CanonicalRequest {
        CanonicalRequest {
            schema_version: 1,
            tenant_id: TenantId::new("tenant-1").unwrap(),
            operation_kind: "create_session".to_string(),
            target: Some(ControlledResourceRef {
                tenant_id: TenantId::new("tenant-1").unwrap(),
                media_session_id: None,
                media_binding_id: None,
                resource_kind: "session".to_string(),
                resource_handle: "h1".to_string(),
                owner_epoch: OwnerEpoch(1),
                node_instance_epoch: cheetah_media_api::ids::MediaNodeInstanceEpoch(42),
                generation: ResourceGeneration(0),
            }),
            media_session_id: None,
            media_binding_id: None,
            business_params: serde_json::json!({
                "z": 1,
                "a": [3, 2, 1],
                "nested": { "b": 2, "a": 1 }
            }),
        }
    }

    #[test]
    fn idempotency_key_rejects_empty_and_control_chars() {
        let empty = IdempotencyKey::new(TenantId::new("tenant-1").unwrap(), "create_session", "");
        assert!(empty.validate().is_err());

        let ctrl = IdempotencyKey::new(
            TenantId::new("tenant-1").unwrap(),
            "create_session",
            "key\x01value",
        );
        assert!(ctrl.validate().is_err());

        let ok = IdempotencyKey::new(
            TenantId::new("tenant-1").unwrap(),
            "create_session",
            "valid-key",
        );
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn digest_is_stable_for_key_order() {
        let r1 = request();
        let d1 = r1.digest().unwrap();

        let mut r2 = request();
        // Reorder business_params object keys; canonicalization must produce
        // the same digest.
        r2.business_params = serde_json::json!({
            "nested": { "a": 1, "b": 2 },
            "a": [3, 2, 1],
            "z": 1
        });
        let d2 = r2.digest().unwrap();

        assert_eq!(d1, d2);
    }

    #[test]
    fn digest_changes_with_schema_version() {
        let mut r = request();
        let d1 = r.digest().unwrap();
        r.schema_version = 2;
        let d2 = r.digest().unwrap();
        assert_ne!(d1, d2);
    }

    #[test]
    fn digest_changes_with_business_params() {
        let mut r = request();
        let d1 = r.digest().unwrap();
        r.business_params = serde_json::json!({"x": 1});
        let d2 = r.digest().unwrap();
        assert_ne!(d1, d2);
    }

    #[test]
    fn digest_hex_round_trips() {
        let d = request().digest().unwrap();
        let hex = d.to_hex();
        assert_eq!(hex.len(), 64);
    }
}
