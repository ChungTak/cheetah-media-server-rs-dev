//! RTP module operator metrics.
//!
//! Provides a cheap, lock-free aggregator for Prometheus-style counters and a
//! snapshot type that can be rendered by the module's HTTP/admin surface.
//! When an engine `MetricsApi` is supplied, counters are also emitted to the
//! shared engine registry under the `rtp_` namespace.
//!
//! 提供廉价的 Prometheus 风格计数器聚合器与可由模块 HTTP/管理面渲染的快照类型。
//! 当提供引擎 `MetricsApi` 时，计数器也会以 `rtp_` 前缀写入共享引擎注册表。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cheetah_sdk::MetricsApi;
use serde::Serialize;

const REQUESTED: &str = "rtp_sessions_requested_total";
const OPENED: &str = "rtp_sessions_opened_total";
const FAILED: &str = "rtp_sessions_failed_total";
const CLOSED: &str = "rtp_sessions_closed_total";
const RATE_LIMITED: &str = "rtp_sessions_rate_limited_total";
const ADMISSION_DENIED: &str = "rtp_sessions_admission_denied_total";
const ROLLBACK: &str = "rtp_rollback_total";
const RECONCILE_ORPHANS: &str = "rtp_reconcile_orphans_stopped_total";
const ACTIVE: &str = "rtp_sessions_active";

/// Monotonic counters and gauges for the RTP module.
///
/// Counters are atomics so they can be bumped from the request hot path without
/// holding the session registry lock. The active session gauge is filled in at
/// snapshot time from the orchestrator's live session count.
///
/// RTP 模块的单调计数器和仪表盘。
#[derive(Default)]
pub struct RtpModuleMetrics {
    sessions_requested_total: AtomicU64,
    sessions_opened_total: AtomicU64,
    sessions_failed_total: AtomicU64,
    sessions_closed_total: AtomicU64,
    sessions_rate_limited_total: AtomicU64,
    sessions_admission_denied_total: AtomicU64,
    rollback_total: AtomicU64,
    reconcile_orphans_stopped_total: AtomicU64,
    api: Option<Arc<dyn MetricsApi>>,
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

impl std::fmt::Debug for RtpModuleMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtpModuleMetrics")
            .field("api", &self.api.is_some())
            .finish_non_exhaustive()
    }
}

impl RtpModuleMetrics {
    /// Create a new metrics aggregator wrapped in an `Arc`, optionally wired
    /// to the shared engine `MetricsApi`.
    pub fn new(api: Option<Arc<dyn MetricsApi>>) -> Arc<Self> {
        Arc::new(Self {
            api,
            ..Self::default()
        })
    }

    fn inc(counter: &AtomicU64, api: &Option<Arc<dyn MetricsApi>>, key: &str) {
        counter.fetch_add(1, Ordering::Relaxed);
        if let Some(api) = api {
            api.inc(key, 1);
        }
    }

    fn inc_n(counter: &AtomicU64, api: &Option<Arc<dyn MetricsApi>>, key: &str, n: u64) {
        if n > 0 {
            counter.fetch_add(n, Ordering::Relaxed);
            if let Some(api) = api {
                api.inc(key, n);
            }
        }
    }

    fn set_active(api: &Option<Arc<dyn MetricsApi>>, value: u64) {
        if let Some(api) = api {
            api.set(ACTIVE, value);
        }
    }

    /// Increment the `sessions_requested_total` counter.
    pub fn inc_session_requested(&self) {
        Self::inc(&self.sessions_requested_total, &self.api, REQUESTED);
    }

    /// Increment the `sessions_opened_total` counter and update the active gauge.
    pub fn inc_session_opened(&self, active: u64) {
        Self::inc(&self.sessions_opened_total, &self.api, OPENED);
        Self::set_active(&self.api, active);
    }

    /// Increment the `sessions_failed_total` counter.
    pub fn inc_session_failed(&self) {
        Self::inc(&self.sessions_failed_total, &self.api, FAILED);
    }

    /// Increment the `sessions_closed_total` counter and update the active gauge.
    pub fn inc_session_closed(&self, active: u64) {
        Self::inc(&self.sessions_closed_total, &self.api, CLOSED);
        Self::set_active(&self.api, active);
    }

    /// Increment the `sessions_rate_limited_total` counter.
    pub fn inc_rate_limited(&self) {
        Self::inc(&self.sessions_rate_limited_total, &self.api, RATE_LIMITED);
    }

    /// Increment the `sessions_admission_denied_total` counter.
    pub fn inc_admission_denied(&self) {
        Self::inc(
            &self.sessions_admission_denied_total,
            &self.api,
            ADMISSION_DENIED,
        );
    }

    /// Increment the `rollback_total` counter.
    pub fn inc_rollback(&self) {
        Self::inc(&self.rollback_total, &self.api, ROLLBACK);
    }

    /// Increment the `reconcile_orphans_stopped_total` counter by `n`.
    pub fn inc_reconcile_orphans(&self, n: u64, active: u64) {
        Self::inc_n(
            &self.reconcile_orphans_stopped_total,
            &self.api,
            RECONCILE_ORPHANS,
            n,
        );
        Self::set_active(&self.api, active);
    }

    /// Build a snapshot, filling the active-session gauge from the caller.
    ///
    /// `sessions_active` should be obtained from the orchestrator's live
    /// session count so the gauge reflects the current registry state.
    pub fn snapshot(&self, sessions_active: u64) -> RtpModuleMetricsSnapshot {
        Self::set_active(&self.api, sessions_active);
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
        let m = RtpModuleMetrics::new(None);
        m.inc_session_requested();
        m.inc_session_requested();
        m.inc_session_opened(1);
        m.inc_session_failed();
        m.inc_session_closed(0);
        m.inc_rate_limited();
        m.inc_admission_denied();
        m.inc_rollback();
        m.inc_reconcile_orphans(3, 0);

        let snap = m.snapshot(0);
        assert_eq!(snap.sessions_requested_total, 2);
        assert_eq!(snap.sessions_opened_total, 1);
        assert_eq!(snap.sessions_failed_total, 1);
        assert_eq!(snap.sessions_closed_total, 1);
        assert_eq!(snap.sessions_active, 0);
        assert_eq!(snap.sessions_rate_limited_total, 1);
        assert_eq!(snap.sessions_admission_denied_total, 1);
        assert_eq!(snap.rollback_total, 1);
        assert_eq!(snap.reconcile_orphans_stopped_total, 3);
    }
}
