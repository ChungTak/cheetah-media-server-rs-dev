//! RTP module operator metrics.
//!
//! Provides a cheap, lock-free aggregator for Prometheus-style counters and a
//! snapshot type that can be rendered by the module's HTTP/admin surface.
//!
//! 提供廉价的 Prometheus 风格计数器聚合器与可由模块 HTTP/管理面渲染的快照类型。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;

/// Monotonic counters and gauges for the RTP module.
///
/// Counters are atomics so they can be bumped from the request hot path without
/// holding the session registry lock. The active session gauge is filled in at
/// snapshot time from the orchestrator's live session count.
///
/// RTP 模块的单调计数器和仪表盘。
#[derive(Debug, Default)]
pub struct RtpModuleMetrics {
    sessions_requested_total: AtomicU64,
    sessions_opened_total: AtomicU64,
    sessions_failed_total: AtomicU64,
    sessions_closed_total: AtomicU64,
    sessions_rate_limited_total: AtomicU64,
    sessions_admission_denied_total: AtomicU64,
    rollback_total: AtomicU64,
    reconcile_orphans_stopped_total: AtomicU64,
}

/// Read-only snapshot of `RtpModuleMetrics` for rendering and serialization.
///
/// 用于渲染与序列化的 `RtpModuleMetrics` 只读快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RtpModuleMetricsSnapshot {
    pub sessions_requested_total: u64,
    pub sessions_opened_total: u64,
    pub sessions_failed_total: u64,
    pub sessions_closed_total: u64,
    pub sessions_active: u64,
    pub sessions_rate_limited_total: u64,
    pub sessions_admission_denied_total: u64,
    pub rollback_total: u64,
    pub reconcile_orphans_stopped_total: u64,
}

impl RtpModuleMetrics {
    /// Create a new metrics aggregator wrapped in an `Arc`.
    ///
    /// 创建包装在 Arc 中的新指标聚合器。
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Increment the `sessions_requested_total` counter.
    pub fn inc_session_requested(&self) {
        self.sessions_requested_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the `sessions_opened_total` counter.
    pub fn inc_session_opened(&self) {
        self.sessions_opened_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the `sessions_failed_total` counter.
    pub fn inc_session_failed(&self) {
        self.sessions_failed_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the `sessions_closed_total` counter.
    pub fn inc_session_closed(&self) {
        self.sessions_closed_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the `sessions_rate_limited_total` counter.
    pub fn inc_rate_limited(&self) {
        self.sessions_rate_limited_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the `sessions_admission_denied_total` counter.
    pub fn inc_admission_denied(&self) {
        self.sessions_admission_denied_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the `rollback_total` counter.
    pub fn inc_rollback(&self) {
        self.rollback_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the `reconcile_orphans_stopped_total` counter by `n`.
    pub fn inc_reconcile_orphans(&self, n: u64) {
        if n > 0 {
            self.reconcile_orphans_stopped_total
                .fetch_add(n, Ordering::Relaxed);
        }
    }

    /// Build a snapshot, filling the active-session gauge from the caller.
    ///
    /// `sessions_active` should be obtained from the orchestrator's live
    /// session count so the gauge reflects the current registry state.
    pub fn snapshot(&self, sessions_active: u64) -> RtpModuleMetricsSnapshot {
        RtpModuleMetricsSnapshot {
            sessions_requested_total: self.sessions_requested_total.load(Ordering::Relaxed),
            sessions_opened_total: self.sessions_opened_total.load(Ordering::Relaxed),
            sessions_failed_total: self.sessions_failed_total.load(Ordering::Relaxed),
            sessions_closed_total: self.sessions_closed_total.load(Ordering::Relaxed),
            sessions_active,
            sessions_rate_limited_total: self.sessions_rate_limited_total.load(Ordering::Relaxed),
            sessions_admission_denied_total: self
                .sessions_admission_denied_total
                .load(Ordering::Relaxed),
            rollback_total: self.rollback_total.load(Ordering::Relaxed),
            reconcile_orphans_stopped_total: self
                .reconcile_orphans_stopped_total
                .load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_snapshot() {
        let m = RtpModuleMetrics::default();
        m.inc_session_requested();
        m.inc_session_requested();
        m.inc_session_opened();
        m.inc_session_failed();
        m.inc_session_closed();
        m.inc_rate_limited();
        m.inc_admission_denied();
        m.inc_rollback();
        m.inc_reconcile_orphans(3);

        let snap = m.snapshot(5);
        assert_eq!(snap.sessions_requested_total, 2);
        assert_eq!(snap.sessions_opened_total, 1);
        assert_eq!(snap.sessions_failed_total, 1);
        assert_eq!(snap.sessions_closed_total, 1);
        assert_eq!(snap.sessions_active, 5);
        assert_eq!(snap.sessions_rate_limited_total, 1);
        assert_eq!(snap.sessions_admission_denied_total, 1);
        assert_eq!(snap.rollback_total, 1);
        assert_eq!(snap.reconcile_orphans_stopped_total, 3);
    }
}
