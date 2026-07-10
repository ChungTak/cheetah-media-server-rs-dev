use std::sync::atomic::{AtomicU64, Ordering};

use cheetah_codec::{RepairEventCounters, RuntimeObservabilityReport};
use cheetah_sdk::MetricsApi;
use dashmap::DashMap;

/// Process-wide metrics registry.
///
/// Two value kinds are tracked:
/// * **counters** — monotonic, additive (e.g. layer-classified repair events,
///   `SystemArchitecture.md` §4). Fed with per-observation deltas.
/// * **gauges** — latest-snapshot floating point values (e.g. the §4 runtime
///   report timing baseline). Overwritten on each publish.
#[derive(Default)]
pub struct MetricsRegistry {
    counters: DashMap<String, AtomicU64>,
    gauges: DashMap<String, AtomicU64>,
}

impl MetricsRegistry {
    /// Increments the value.
    /// 递增该值。
    pub fn inc(&self, key: &str, value: u64) {
        let entry = self
            .counters
            .entry(key.to_string())
            .or_insert_with(|| AtomicU64::new(0));
        entry.fetch_add(value, Ordering::Relaxed);
    }

    /// Overwrite a gauge with the latest floating-point snapshot value.
    pub fn set_gauge(&self, key: &str, value: f64) {
        let entry = self
            .gauges
            .entry(key.to_string())
            .or_insert_with(|| AtomicU64::new(0));
        entry.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Add a batch of layer-classified repair events to the monotonic counters
    /// (`source_repair_events` / `canonical_repair_events` /
    /// `egress_repair_events`), per `SystemArchitecture.md` §4.
    ///
    /// The argument is treated as a delta to add; callers feed the increments
    /// observed since their last publish.
    pub fn record_repair_events(&self, delta: &RepairEventCounters) {
        self.inc("source_repair_events", delta.source_repair_events);
        self.inc("canonical_repair_events", delta.canonical_repair_events);
        self.inc("egress_repair_events", delta.egress_repair_events);
    }

    /// Publish the latest per-session runtime observability report timing
    /// baseline as gauges (§4). Repair counters are recorded separately via
    /// [`MetricsRegistry::record_repair_events`] so counter/gauge semantics stay
    /// unambiguous.
    pub fn record_runtime_report(&self, report: &RuntimeObservabilityReport) {
        if let Some(v) = report.startup_latency_ms {
            self.set_gauge("startup_latency_ms", v);
        }
        if let Some(v) = report.first_second_avg_frame_interval_ms {
            self.set_gauge("first_second_avg_frame_interval_ms", v);
        }
        if let Some(v) = report.average_playback_rate_x {
            self.set_gauge("average_playback_rate_x", v);
        }
        if let Some(v) = report.first_keyframe_delay_ms {
            self.set_gauge("first_keyframe_delay_ms", v);
        }
    }
}

impl MetricsApi for MetricsRegistry {
    fn render(&self) -> String {
        let mut out = String::new();
        for item in self.counters.iter() {
            let value = item.value().load(Ordering::Relaxed);
            out.push_str(&format!("{} {}\n", item.key(), value));
        }
        for item in self.gauges.iter() {
            let value = f64::from_bits(item.value().load(Ordering::Relaxed));
            out.push_str(&format!("{} {}\n", item.key(), value));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cheetah_codec::RepairLayer;

    #[test]
    fn repair_events_accumulate_by_layer() {
        let reg = MetricsRegistry::default();
        let mut delta = RepairEventCounters::default();
        delta.record_layer(RepairLayer::Source);
        delta.record_layer(RepairLayer::Canonical);
        delta.record_layer(RepairLayer::Canonical);
        reg.record_repair_events(&delta);
        reg.record_repair_events(&delta);

        let rendered = reg.render();
        assert!(rendered.contains("source_repair_events 2"));
        assert!(rendered.contains("canonical_repair_events 4"));
        assert!(rendered.contains("egress_repair_events 0"));
    }

    #[test]
    fn runtime_report_gauges_are_overwritten() {
        let reg = MetricsRegistry::default();
        reg.record_runtime_report(&RuntimeObservabilityReport {
            startup_latency_ms: Some(20.0),
            first_keyframe_delay_ms: Some(40.0),
            ..Default::default()
        });
        reg.record_runtime_report(&RuntimeObservabilityReport {
            startup_latency_ms: Some(25.0),
            ..Default::default()
        });

        let rendered = reg.render();
        assert!(rendered.contains("startup_latency_ms 25"));
        // first_keyframe_delay_ms was only set once; latest publish left it.
        assert!(rendered.contains("first_keyframe_delay_ms 40"));
    }
}
