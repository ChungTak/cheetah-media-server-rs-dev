use std::sync::atomic::{AtomicU64, Ordering};

use cheetah_sdk::MetricsApi;
use dashmap::DashMap;

#[derive(Default)]
pub struct MetricsRegistry {
    counters: DashMap<String, AtomicU64>,
}

impl MetricsRegistry {
    pub fn inc(&self, key: &str, value: u64) {
        let entry = self
            .counters
            .entry(key.to_string())
            .or_insert_with(|| AtomicU64::new(0));
        entry.fetch_add(value, Ordering::Relaxed);
    }
}

impl MetricsApi for MetricsRegistry {
    fn render(&self) -> String {
        let mut out = String::new();
        for item in self.counters.iter() {
            let value = item.value().load(Ordering::Relaxed);
            out.push_str(&format!("{} {}\n", item.key(), value));
        }
        out
    }
}
