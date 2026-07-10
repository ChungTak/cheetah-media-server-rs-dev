use std::sync::atomic::{AtomicBool, Ordering};

use cheetah_sdk::HealthApi;

/// `HealthService` data structure.
/// `HealthService` 数据结构.
#[derive(Default)]
pub struct HealthService {
    /// `live` field of type `AtomicBool`.
    /// `live` 字段，类型为 `AtomicBool`.
    live: AtomicBool,
    /// `ready` field of type `AtomicBool`.
    /// `ready` 字段，类型为 `AtomicBool`.
    ready: AtomicBool,
}

impl HealthService {
    /// Sets the `live` value.
    /// Sets `live` 值.
    pub fn set_live(&self, value: bool) {
        self.live.store(value, Ordering::Release);
    }

    /// Sets the `ready` value.
    /// Sets `ready` 值.
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
