use std::sync::atomic::{AtomicBool, Ordering};

use cheetah_sdk::HealthApi;

#[derive(Default)]
pub struct HealthService {
    live: AtomicBool,
    ready: AtomicBool,
}

impl HealthService {
    pub fn set_live(&self, value: bool) {
        self.live.store(value, Ordering::Release);
    }

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
