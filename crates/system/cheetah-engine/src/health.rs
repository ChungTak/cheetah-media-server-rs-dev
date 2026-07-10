use std::sync::atomic::{AtomicBool, Ordering};

use cheetah_sdk::HealthApi;

/// Service contract for `Health`.
/// `Health` 的服务契约。
#[derive(Default)]
pub struct HealthService {
    live: AtomicBool,
    ready: AtomicBool,
}

impl HealthService {
    /// Sets the `live` value.
    /// 设置 `live` 的值。
    pub fn set_live(&self, value: bool) {
        self.live.store(value, Ordering::Release);
    }

    /// Sets the `ready` value.
    /// 设置 `ready` 的值。
    pub fn set_ready(&self, value: bool) {
        self.ready.store(value, Ordering::Release);
    }
}

impl HealthApi for HealthService {
    fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}
