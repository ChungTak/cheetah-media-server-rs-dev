//! In-memory capacity orchestrator.
//!
//! 容量编排器：原子地检查硬上限、发放 RAII 许可，并在 drop 时释放。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cheetah_media_api::capacity::{
    CapacityLimits, CapacityPermit, CapacityRequest, CapacitySnapshot, CapacityVector,
};
use cheetah_media_api::error::{MediaError, MediaErrorCode};
use cheetah_media_api::port::MediaCapacityApi;

use crate::store::now_ms;

/// In-memory capacity manager that enforces hard per-dimension limits and a
/// node-level create gate.
///
/// 内存中的容量管理器，按维度硬上限与节点级创建门控进行限制。
#[derive(Debug, Clone)]
pub struct CapacityOrchestrator {
    inner: Arc<Mutex<CapacityState>>,
}

#[derive(Debug)]
struct CapacityState {
    used: CapacityVector,
    limits: CapacityLimits,
    node_gate_open: bool,
    updated_at_ms: i64,
}

impl CapacityOrchestrator {
    /// Create a new orchestrator with the given hard limits.
    ///
    /// 使用给定硬上限创建容量编排器。
    pub fn new(limits: CapacityLimits) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CapacityState {
                used: CapacityVector::default(),
                limits,
                node_gate_open: true,
                updated_at_ms: now_ms(),
            })),
        }
    }

    fn snapshot_locked(state: &CapacityState) -> CapacitySnapshot {
        CapacitySnapshot {
            used: state.used.clone(),
            remaining: remaining(&state.used, &state.limits),
            node_gate_open: state.node_gate_open,
            updated_at_ms: state.updated_at_ms,
        }
    }
}

#[async_trait]
impl MediaCapacityApi for CapacityOrchestrator {
    async fn acquire(
        &self,
        request: CapacityRequest,
    ) -> cheetah_media_api::error::Result<Box<dyn CapacityPermit>> {
        let mut state = self.inner.lock().unwrap();

        if !state.node_gate_open {
            return Err(
                MediaError::new(MediaErrorCode::Busy, "node create gate is closed")
                    .with_retry_after(100),
            );
        }

        if !fits(&state.used, &request, &state.limits) {
            return Err(
                MediaError::new(MediaErrorCode::Busy, "capacity hard limit reached")
                    .with_retry_after(100),
            );
        }

        state.used = add(&state.used, &request);
        state.updated_at_ms = now_ms();
        let permit = OwnedCapacityPermit {
            inner: Arc::clone(&self.inner),
            request,
            resource_handle: None,
            released: false,
        };
        Ok(Box::new(permit))
    }

    async fn snapshot(&self) -> cheetah_media_api::error::Result<CapacitySnapshot> {
        let state = self.inner.lock().unwrap();
        Ok(CapacityOrchestrator::snapshot_locked(&state))
    }

    async fn update_limits(&self, limits: CapacityLimits) -> cheetah_media_api::error::Result<()> {
        let mut state = self.inner.lock().unwrap();
        state.limits = limits;
        state.updated_at_ms = now_ms();
        Ok(())
    }

    async fn set_node_gate(&self, open: bool) -> cheetah_media_api::error::Result<()> {
        let mut state = self.inner.lock().unwrap();
        state.node_gate_open = open;
        state.updated_at_ms = now_ms();
        Ok(())
    }
}

/// Owned capacity permit that releases its reservation when dropped.
///
/// 持有的容量许可，在 drop 时释放预留。
#[derive(Debug)]
struct OwnedCapacityPermit {
    inner: Arc<Mutex<CapacityState>>,
    request: CapacityVector,
    resource_handle: Option<String>,
    released: bool,
}

impl CapacityPermit for OwnedCapacityPermit {
    fn resource_handle(&self) -> Option<&str> {
        self.resource_handle.as_deref()
    }
}

impl OwnedCapacityPermit {
    fn release(&mut self) {
        if self.released {
            return;
        }
        let mut state = self.inner.lock().unwrap();
        state.used = sub(&state.used, &self.request);
        state.updated_at_ms = now_ms();
        self.released = true;
    }
}

impl Drop for OwnedCapacityPermit {
    fn drop(&mut self) {
        self.release();
    }
}

fn fits(used: &CapacityVector, request: &CapacityVector, limits: &CapacityLimits) -> bool {
    used.session_count + request.session_count <= limits.session_count
        && used.port_count + request.port_count <= limits.port_count
        && used.bandwidth_bps + request.bandwidth_bps <= limits.bandwidth_bps
        && used.worker_count + request.worker_count <= limits.worker_count
        && used.blocking_job_count + request.blocking_job_count <= limits.blocking_job_count
        && used.file_task_count + request.file_task_count <= limits.file_task_count
        && used.event_subscriber_count + request.event_subscriber_count
            <= limits.event_subscriber_count
        && used.cpu_permille + request.cpu_permille <= limits.cpu_permille
}

fn add(a: &CapacityVector, b: &CapacityVector) -> CapacityVector {
    CapacityVector {
        session_count: a.session_count + b.session_count,
        port_count: a.port_count + b.port_count,
        bandwidth_bps: a.bandwidth_bps + b.bandwidth_bps,
        worker_count: a.worker_count + b.worker_count,
        blocking_job_count: a.blocking_job_count + b.blocking_job_count,
        file_task_count: a.file_task_count + b.file_task_count,
        event_subscriber_count: a.event_subscriber_count + b.event_subscriber_count,
        cpu_permille: a.cpu_permille + b.cpu_permille,
    }
}

fn sub(a: &CapacityVector, b: &CapacityVector) -> CapacityVector {
    CapacityVector {
        session_count: a.session_count.saturating_sub(b.session_count),
        port_count: a.port_count.saturating_sub(b.port_count),
        bandwidth_bps: a.bandwidth_bps.saturating_sub(b.bandwidth_bps),
        worker_count: a.worker_count.saturating_sub(b.worker_count),
        blocking_job_count: a.blocking_job_count.saturating_sub(b.blocking_job_count),
        file_task_count: a.file_task_count.saturating_sub(b.file_task_count),
        event_subscriber_count: a
            .event_subscriber_count
            .saturating_sub(b.event_subscriber_count),
        cpu_permille: a.cpu_permille.saturating_sub(b.cpu_permille),
    }
}

fn remaining(used: &CapacityVector, limits: &CapacityLimits) -> CapacityVector {
    CapacityVector {
        session_count: limits.session_count.saturating_sub(used.session_count),
        port_count: limits.port_count.saturating_sub(used.port_count),
        bandwidth_bps: limits.bandwidth_bps.saturating_sub(used.bandwidth_bps),
        worker_count: limits.worker_count.saturating_sub(used.worker_count),
        blocking_job_count: limits
            .blocking_job_count
            .saturating_sub(used.blocking_job_count),
        file_task_count: limits.file_task_count.saturating_sub(used.file_task_count),
        event_subscriber_count: limits
            .event_subscriber_count
            .saturating_sub(used.event_subscriber_count),
        cpu_permille: limits.cpu_permille.saturating_sub(used.cpu_permille),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> CapacityLimits {
        CapacityVector {
            session_count: 2,
            port_count: 2,
            bandwidth_bps: 1000,
            worker_count: 1,
            blocking_job_count: 1,
            file_task_count: 1,
            event_subscriber_count: 1,
            cpu_permille: 100,
        }
    }

    fn request_one() -> CapacityRequest {
        CapacityVector {
            session_count: 1,
            port_count: 1,
            bandwidth_bps: 100,
            worker_count: 0,
            blocking_job_count: 0,
            file_task_count: 0,
            event_subscriber_count: 0,
            cpu_permille: 0,
        }
    }

    #[tokio::test]
    async fn acquire_releases_on_drop() {
        let orchestrator = CapacityOrchestrator::new(limits());
        {
            let permit = orchestrator.acquire(request_one()).await.unwrap();
            assert_eq!(permit.resource_handle(), None);
            let snap = orchestrator.snapshot().await.unwrap();
            assert_eq!(snap.used.session_count, 1);
            assert_eq!(snap.remaining.session_count, 1);
        }
        let snap = orchestrator.snapshot().await.unwrap();
        assert_eq!(snap.used.session_count, 0);
        assert_eq!(snap.remaining.session_count, 2);
    }

    #[tokio::test]
    async fn hard_limit_rejects_over_commit() {
        let orchestrator = CapacityOrchestrator::new(limits());
        let _p1 = orchestrator.acquire(request_one()).await.unwrap();
        let _p2 = orchestrator.acquire(request_one()).await.unwrap();
        let err = orchestrator.acquire(request_one()).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Busy);
        assert_eq!(
            err.outcome,
            cheetah_media_api::error::EffectOutcome::NotApplied
        );
    }

    #[tokio::test]
    async fn closed_node_gate_rejects_acquire() {
        let orchestrator = CapacityOrchestrator::new(limits());
        orchestrator.set_node_gate(false).await.unwrap();
        let err = orchestrator.acquire(request_one()).await.unwrap_err();
        assert_eq!(err.code, MediaErrorCode::Busy);
    }

    #[tokio::test]
    async fn update_limits_allows_more_after_increase() {
        let orchestrator = CapacityOrchestrator::new(limits());
        let _p1 = orchestrator.acquire(request_one()).await.unwrap();
        let _p2 = orchestrator.acquire(request_one()).await.unwrap();

        let mut new_limits = limits();
        new_limits.session_count = 3;
        new_limits.port_count = 3;
        orchestrator.update_limits(new_limits).await.unwrap();

        let _p3 = orchestrator.acquire(request_one()).await.unwrap();
        let snap = orchestrator.snapshot().await.unwrap();
        assert_eq!(snap.used.session_count, 3);
    }
}
