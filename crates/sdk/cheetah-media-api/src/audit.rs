//! Framework-neutral audit logging types and API.
//!
//! 框架无关的审计日志类型与 API。

use async_trait::async_trait;

use crate::port::MediaRequestContext;

/// Result of an audited operation.
///
/// 被审计操作的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditResult {
    Success,
    Failure { code: String, message: String },
    Denied { reason: String },
}

/// A single audit record.
///
/// 一条审计记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    /// Wall-clock timestamp in milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
    /// Request identifier carried from `MediaRequestContext`.
    pub request_id: String,
    /// Optional correlation identifier.
    pub correlation_id: Option<String>,
    /// Identity of the principal that performed the operation.
    pub principal: Option<String>,
    /// Operation name, e.g. `media.close` or `record.start`.
    pub operation: String,
    /// Resource identifier affected by the operation.
    pub resource: String,
    /// Outcome of the operation.
    pub result: AuditResult,
    /// Safe, non-sensitive summary of the operation.
    pub summary: String,
}

/// Audit logging API.
///
/// 审计日志 API。
#[async_trait]
pub trait AuditApi: Send + Sync {
    /// Record an audit event.
    ///
    /// Implementations must redact tokens, secrets, passwords, full SDP bodies,
    /// and credential-bearing URLs before persisting or emitting the record.
    async fn record(&self, ctx: &MediaRequestContext, event: AuditEvent);
}
