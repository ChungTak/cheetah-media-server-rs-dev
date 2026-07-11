use std::sync::atomic::{AtomicBool, Ordering};

use cheetah_sdk::HealthApi;

/// In-memory liveness/readiness probe state.
///
/// 内存存活/就绪探针状态。
#[derive(Default)]
pub struct HealthService {
    live: AtomicBool,
    ready: AtomicBool,
}

impl HealthService {
    /// Set the live probe flag.
    ///
    /// 设置存活探针标志。
    pub fn set_live(&self, value: bool) {
        self.live.store(value, Ordering::Release);
    }

    /// Set the ready probe flag.
    ///
    /// 设置就绪探针标志。
    pub fn set_ready(&self, value: bool) {
        self.ready.store(value, Ordering::Release);
    }
}

/// `HealthApi` implementation backed by atomic booleans.
///
/// 由原子布尔支持的 `HealthApi` 实现。
impl HealthApi for HealthService {
    fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}
