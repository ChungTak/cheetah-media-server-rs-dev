//! Administrative control-plane operations protected by mTLS admin scope and audit.
//!
//! 受 mTLS admin scope/audit 保护的控制面管理操作。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::MediaError;
use crate::ids::{MediaNodeId, TenantId};

/// Scope required to perform an admin operation.
///
/// admin 操作所需的 mTLS scope。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminScope {
    /// Drain / isolate / leave drain.
    Node,
    /// Trigger reconciliation and inspect safe diagnostics.
    Reconcile,
    /// Rotate TLS identity or cursor HMAC key.
    Tls,
    /// Compact or checkpoint the SQLite store.
    Store,
    /// Clean up typed orphan resources.
    Orphan,
}

/// Administrative identity derived from mTLS client certificate metadata.
///
/// 从 mTLS 客户端证书元数据提取的管理身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminIdentity {
    pub common_name: String,
    pub scopes: Vec<AdminScope>,
}

impl AdminIdentity {
    /// Check whether the identity carries a required scope.
    pub fn has_scope(&self, scope: AdminScope) -> bool {
        self.scopes.contains(&scope)
    }
}

/// Request to put a node into or take it out of drain mode.
///
/// 节点进入/离开 drain 模式请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainNodeRequest {
    pub node_id: MediaNodeId,
    pub drain: bool,
    #[serde(default)]
    pub reason: String,
}

/// Response to a drain-node request.
///
/// drain 节点响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainNodeResponse {
    pub node_id: MediaNodeId,
    pub draining: bool,
}

/// Scope of a reconciliation run.
///
/// 对账运行范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileScope {
    #[default]
    All,
    Node,
    Tenant,
}

/// Request to trigger a reconciliation pass.
///
/// 触发对账请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerReconciliationRequest {
    pub scope: ReconcileScope,
    pub node_id: Option<MediaNodeId>,
    pub tenant_id: Option<TenantId>,
}

/// Response to a reconciliation trigger.
///
/// 触发对账响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerReconciliationResponse {
    pub triggered: bool,
}

/// Request for safe node/store/event diagnostics.
///
/// 安全诊断请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsRequest {
    pub tenant_id: Option<TenantId>,
    pub resource_kind: Option<String>,
    /// Maximum number of recent event records to summarize.
    #[serde(default = "default_max_events")]
    pub max_events: u32,
}

fn default_max_events() -> u32 {
    100
}

/// Safe diagnostic summary. Contains counts and high-level health only; no secrets,
/// raw rows, or arbitrary files.
///
/// 安全诊断摘要，仅包含计数与高层健康信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsResponse {
    pub node_count: u64,
    pub resource_count: u64,
    pub event_count: u64,
    pub non_terminal_resource_count: u64,
}

/// Component whose TLS material or HMAC key should be rotated.
///
/// 需要轮换 TLS 材料或 HMAC key 的组件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsComponent {
    GrpcListener,
    CursorKey,
}

/// Request to rotate TLS material or a cursor HMAC key.
///
/// 轮换 TLS 材料或 cursor key 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotateTlsRequest {
    pub component: TlsComponent,
}

/// Response to a TLS/cursor-key rotation request.
///
/// 轮换响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotateTlsResponse {
    pub applied: bool,
}

/// Request to compact or checkpoint the durable store.
///
/// 压缩或 checkpoint store 请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointKind {
    #[default]
    Compact,
    Checkpoint,
}

/// Request to compact or checkpoint the store.
///
/// store 压缩/checkpoint 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointStoreRequest {
    pub kind: CheckpointKind,
}

/// Response to a store checkpoint request.
///
/// store checkpoint 响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointStoreResponse {
    pub applied: bool,
}

/// Request to clean up a typed orphan resource.
///
/// 清理指定 orphan 资源请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupOrphanRequest {
    pub tenant_id: TenantId,
    pub resource_kind: String,
    pub resource_handle: String,
    #[serde(default)]
    pub reason: String,
}

/// Response to an orphan cleanup request.
///
/// orphan 清理响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupOrphanResponse {
    pub cleaned: bool,
}

/// Administrative control-plane API.
///
/// 控制面管理 API。
#[async_trait]
pub trait AdminApi: Send + Sync {
    /// Put a node into or take it out of drain mode.
    async fn drain_node(
        &self,
        identity: &AdminIdentity,
        request: DrainNodeRequest,
    ) -> Result<DrainNodeResponse, MediaError>;

    /// Trigger a reconciliation pass.
    async fn trigger_reconciliation(
        &self,
        identity: &AdminIdentity,
        request: TriggerReconciliationRequest,
    ) -> Result<TriggerReconciliationResponse, MediaError>;

    /// Return safe node/store/event diagnostics.
    async fn inspect_diagnostics(
        &self,
        identity: &AdminIdentity,
        request: DiagnosticsRequest,
    ) -> Result<DiagnosticsResponse, MediaError>;

    /// Rotate TLS listener identity or cursor HMAC key.
    async fn rotate_tls(
        &self,
        identity: &AdminIdentity,
        request: RotateTlsRequest,
    ) -> Result<RotateTlsResponse, MediaError>;

    /// Compact or checkpoint the durable store.
    async fn checkpoint_store(
        &self,
        identity: &AdminIdentity,
        request: CheckpointStoreRequest,
    ) -> Result<CheckpointStoreResponse, MediaError>;

    /// Clean up a typed orphan resource.
    async fn cleanup_orphan(
        &self,
        identity: &AdminIdentity,
        request: CleanupOrphanRequest,
    ) -> Result<CleanupOrphanResponse, MediaError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_identity_checks_scope() {
        let id = AdminIdentity {
            common_name: "ops-1".to_string(),
            scopes: vec![AdminScope::Node, AdminScope::Store],
        };
        assert!(id.has_scope(AdminScope::Node));
        assert!(id.has_scope(AdminScope::Store));
        assert!(!id.has_scope(AdminScope::Tls));
    }

    #[test]
    fn diagnostics_request_defaults_max_events() {
        let req = DiagnosticsRequest {
            tenant_id: None,
            resource_kind: None,
            max_events: default_max_events(),
        };
        assert_eq!(req.max_events, 100);
    }
}
