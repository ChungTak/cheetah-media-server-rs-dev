//! Credential lease and exchange types.
//!
//! 凭据租借与交换类型。

use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use crate::ids::{
    CredentialHandle, MediaBindingId, MediaNodeInstanceEpoch, OperationStepId, TenantId,
};

/// Scoped material returned by a credential exchange.
///
/// `CredentialLease` must not be serialized, cloned into persistent objects, or
/// included in events/errors. `Debug` and `Display` are redacted; `Drop` zeroes
/// the secret bytes.
///
/// 凭据交换返回的受限素材。
#[derive(PartialEq, Eq)]
pub enum CredentialMaterial {
    /// Username and password for basic/digest-style authentication.
    UsernamePassword {
        username: String,
        password: Zeroizing<String>,
    },
    /// Bearer token (e.g. OAuth/RTSP Authorization).
    Bearer { token: Zeroizing<String> },
}

impl fmt::Debug for CredentialMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CredentialMaterial::UsernamePassword { username, .. } => f
                .debug_struct("UsernamePassword")
                .field("username", &username)
                .field("password", &"[REDACTED]")
                .finish(),
            CredentialMaterial::Bearer { .. } => f
                .debug_struct("Bearer")
                .field("token", &"[REDACTED]")
                .finish(),
        }
    }
}

impl Drop for CredentialMaterial {
    fn drop(&mut self) {
        match self {
            CredentialMaterial::UsernamePassword { password, .. } => password.zeroize(),
            CredentialMaterial::Bearer { token } => token.zeroize(),
        }
    }
}

/// A short-lived credential lease bound to a tenant, operation step, and
/// resource purpose.
///
/// 短生存期、绑定 tenant/operation step/资源用途的凭据租借。
pub struct CredentialLease {
    pub handle: CredentialHandle,
    pub tenant_id: TenantId,
    pub media_binding_id: Option<MediaBindingId>,
    pub operation_step_id: OperationStepId,
    pub media_node_instance_epoch: MediaNodeInstanceEpoch,
    pub purpose: String,
    pub ttl_ms: u64,
    pub material: CredentialMaterial,
}

impl fmt::Debug for CredentialLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialLease")
            .field("handle", &self.handle)
            .field("tenant_id", &self.tenant_id)
            .field("media_binding_id", &self.media_binding_id)
            .field("operation_step_id", &self.operation_step_id)
            .field("media_node_instance_epoch", &self.media_node_instance_epoch)
            .field("purpose", &self.purpose)
            .field("ttl_ms", &self.ttl_ms)
            .field("material", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for CredentialLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CredentialLease({}, purpose={}, ttl={}ms)",
            self.handle, self.purpose, self.ttl_ms
        )
    }
}

impl Drop for CredentialLease {
    fn drop(&mut self) {
        self.purpose.zeroize();
    }
}

/// Opaque purpose tag used to scope a credential exchange.
///
/// 用于限定凭据交换用途的不透明标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialPurpose(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(material: CredentialMaterial) -> CredentialLease {
        CredentialLease {
            handle: CredentialHandle::new("cred-1").unwrap(),
            tenant_id: TenantId::new("tenant-1").unwrap(),
            media_binding_id: None,
            operation_step_id: OperationStepId::new("550e8400-e29b-41d4-a716-446655440000")
                .unwrap(),
            media_node_instance_epoch: MediaNodeInstanceEpoch(1),
            purpose: "proxy".to_string(),
            ttl_ms: 60_000,
            material,
        }
    }

    #[test]
    fn debug_redacts_password_and_token() {
        let l = lease(CredentialMaterial::UsernamePassword {
            username: "user".to_string(),
            password: Zeroizing::new("secret".to_string()),
        });
        let s = format!("{:?}", l);
        assert!(!s.contains("secret"));
        assert!(s.contains("[REDACTED]"));
    }

    #[test]
    fn display_redacts_material() {
        let l = lease(CredentialMaterial::Bearer {
            token: Zeroizing::new("bearer-token".to_string()),
        });
        let s = l.to_string();
        assert!(!s.contains("bearer-token"));
        assert!(s.contains("<redacted>"));
        assert!(s.contains("proxy"));
    }
}
