//! Default audit logging implementations.
//!
//! 默认审计日志实现。

use async_trait::async_trait;
use cheetah_media_api::audit::{AuditApi, AuditEvent};
use cheetah_media_api::port::MediaRequestContext;

/// Audit implementation that emits structured tracing events.
///
/// 输出结构化 tracing 事件的审计实现。
pub struct TracingAuditApi;

impl TracingAuditApi {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TracingAuditApi {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditApi for TracingAuditApi {
    async fn record(&self, _ctx: &MediaRequestContext, event: AuditEvent) {
        let result = match &event.result {
            cheetah_media_api::audit::AuditResult::Success => "success",
            cheetah_media_api::audit::AuditResult::Failure { code, .. } => code.as_str(),
            cheetah_media_api::audit::AuditResult::Denied { .. } => "denied",
        };
        tracing::info!(
            timestamp_ms = event.timestamp_ms,
            request_id = %event.request_id,
            correlation_id = ?event.correlation_id,
            principal = ?event.principal,
            service = %event.service,
            method = %event.method,
            operation = %event.operation,
            operation_step = ?event.operation_step,
            resource_kind = %event.resource_kind,
            resource = %event.resource,
            result = %result,
            summary = %event.summary,
            node_state = ?event.node_state,
            owner_epoch = ?event.owner_epoch,
            generation = ?event.generation,
            contract_version = ?event.contract_version,
            latency_ms = ?event.latency_ms,
            "audit"
        );
    }
}

/// No-op audit implementation for tests and minimal deployments.
///
/// 用于测试和最小化部署的空审计实现。
pub struct NoopAuditApi;

#[async_trait]
impl AuditApi for NoopAuditApi {
    async fn record(&self, _ctx: &MediaRequestContext, _event: AuditEvent) {}
}
