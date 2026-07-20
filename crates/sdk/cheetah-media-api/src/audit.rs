//! Framework-neutral audit logging types and API.
//!
//! 框架无关的审计日志类型与 API。

use async_trait::async_trait;

use crate::port::MediaRequestContext;

/// Result of an audited operation.
///
/// 被审计操作的结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AuditResult {
    #[default]
    Success,
    Failure {
        code: String,
        message: String,
    },
    Denied {
        reason: String,
    },
}

/// A single structured audit record.
///
/// 一条结构化审计记录。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuditEvent {
    /// Wall-clock timestamp in milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
    /// Request identifier carried from `MediaRequestContext`.
    pub request_id: String,
    /// Optional correlation identifier.
    pub correlation_id: Option<String>,
    /// Identity of the principal that performed the operation.
    pub principal: Option<String>,
    /// Service or module emitting the record, e.g. `cheetah.media`.
    pub service: String,
    /// Method or route being invoked, e.g. `POST /media/close` or gRPC method name.
    pub method: String,
    /// Operation name, e.g. `media.close` or `record.start`.
    pub operation: String,
    /// Operation step for multi-stage mutations, e.g. `validate`, `admission`, `side-effect`.
    pub operation_step: Option<String>,
    /// Resource kind being operated on, e.g. `room`, `stream`, `session`.
    pub resource_kind: String,
    /// Resource identifier affected by the operation.
    pub resource: String,
    /// Outcome of the operation.
    pub result: AuditResult,
    /// Safe, non-sensitive summary of the operation.
    pub summary: String,
    /// Node runtime state at the time of the operation, e.g. `Active` or `Draining`.
    pub node_state: Option<String>,
    /// Owner epoch of the affected resource, if known.
    pub owner_epoch: Option<u64>,
    /// Generation of the affected resource, if known.
    pub generation: Option<u64>,
    /// Signaling contract version the caller claims to support.
    pub contract_version: Option<String>,
    /// End-to-end latency of the operation in milliseconds.
    pub latency_ms: Option<u64>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_event_defaults_and_optional_fields() {
        let event = AuditEvent {
            timestamp_ms: 1,
            request_id: "req-1".to_string(),
            operation: "media.close".to_string(),
            resource: "room/abc".to_string(),
            result: AuditResult::Success,
            summary: "closed".to_string(),
            ..Default::default()
        };

        assert_eq!(event.service, "");
        assert_eq!(event.method, "");
        assert_eq!(event.resource_kind, "");
        assert_eq!(event.operation_step, None);
        assert_eq!(event.contract_version, None);
        assert_eq!(event.latency_ms, None);
    }
}
