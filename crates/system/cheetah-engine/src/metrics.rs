use std::sync::atomic::{AtomicU64, Ordering};

use cheetah_codec::{RepairEventCounters, RuntimeObservabilityReport};
use cheetah_sdk::{MetricLabel, MetricRecord, MetricValue, MetricsApi};
use dashmap::DashMap;

/// Composite key for a labeled metric record.
///
/// 标签化指标记录的复合键。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MetricKey {
    name: String,
    labels: Vec<MetricLabel>,
}

/// Process-wide metrics registry.
///
/// Two value kinds are tracked:
/// * **counters** — monotonic, additive (e.g. layer-classified repair events,
///   `SystemArchitecture.md` §4). Fed with per-observation deltas.
/// * **gauges** — latest-snapshot floating point values (e.g. the §4 runtime
///   report timing baseline). Overwritten on each publish.
/// * **labeled records** — structured observations from `MetricsApi::record`.
#[derive(Default)]
pub struct MetricsRegistry {
    counters: DashMap<String, AtomicU64>,
    gauges: DashMap<String, AtomicU64>,
    records: DashMap<MetricKey, MetricValue>,
}

impl MetricsRegistry {
    /// Increment a counter by the given delta.
    ///
    /// 用给定增量递增计数器。
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

fn merge_metric_value(existing: &mut MetricValue, incoming: &MetricValue) {
    match incoming {
        MetricValue::Counter(b) => {
            if let MetricValue::Counter(a) = existing {
                *a += *b;
            } else {
                *existing = incoming.clone();
            }
        }
        MetricValue::Gauge(b) => {
            *existing = MetricValue::Gauge(*b);
        }
        MetricValue::Histogram { sum, count } => {
            if let MetricValue::Histogram { sum: s, count: c } = existing {
                *s += *sum;
                *c += *count;
            } else {
                *existing = incoming.clone();
            }
        }
    }
}

fn escape_label_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn format_labels(labels: &[MetricLabel]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = labels
        .iter()
        .map(|l| format!("{}=\"{}\"", l.name, escape_label_value(&l.value)))
        .collect();
    format!("{{{}}}", pairs.join(","))
}

/// `MetricsApi` implementation that renders counters and gauges as text.
///
/// `MetricsApi` 实现，将计数器和仪表盘渲染为文本。
impl MetricsApi for MetricsRegistry {
    fn inc(&self, key: &str, value: u64) {
        MetricsRegistry::inc(self, key, value);
    }

    fn set(&self, key: &str, value: u64) {
        MetricsRegistry::set_gauge(self, key, value as f64);
    }

    /// Record a labeled metric observation.
    ///
    /// 记录一条带标签的指标观测。
    fn record(&self, record: MetricRecord) {
        let mut key = MetricKey {
            name: record.name,
            labels: record.labels,
        };
        key.labels.sort_by(|a, b| a.name.cmp(&b.name));
        self.records
            .entry(key)
            .and_modify(|existing| merge_metric_value(existing, &record.value))
            .or_insert(record.value);
    }

    /// Render all counters, gauges, and labeled records as key-value lines.
    ///
    /// 将所有计数器、仪表盘和标签化记录渲染为键值行。
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
        for item in self.records.iter() {
            let (key, value) = item.pair();
            let labels = format_labels(&key.labels);
            match value {
                MetricValue::Counter(v) => {
                    out.push_str(&format!("{}{} {}\n", key.name, labels, v));
                }
                MetricValue::Gauge(v) => {
                    out.push_str(&format!("{}{} {}\n", key.name, labels, v));
                }
                MetricValue::Histogram { sum, count } => {
                    out.push_str(&format!("{}{} {} {}\n", key.name, labels, sum, count));
                }
            }
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

    #[test]
    fn labeled_records_merge_and_render() {
        use cheetah_sdk::{MetricLabel, MetricRecord, MetricValue};

        let reg = MetricsRegistry::default();
        let labels = vec![MetricLabel {
            name: "service".to_string(),
            value: "control_plane".to_string(),
        }];
        reg.record(MetricRecord {
            name: "grpc_requests_total".to_string(),
            labels: labels.clone(),
            value: MetricValue::Counter(5),
            timestamp_ms: 1,
        });
        reg.record(MetricRecord {
            name: "grpc_requests_total".to_string(),
            labels,
            value: MetricValue::Counter(3),
            timestamp_ms: 2,
        });

        let rendered = reg.render();
        assert!(rendered.contains("grpc_requests_total{service=\"control_plane\"} 8"));
    }

    #[test]
    fn label_values_are_escaped_for_prometheus_exposition() {
        use cheetah_sdk::{MetricLabel, MetricRecord, MetricValue};

        let reg = MetricsRegistry::default();
        reg.record(MetricRecord {
            name: "rpc_latency_ms".to_string(),
            labels: vec![MetricLabel {
                name: "path".to_string(),
                value: "a\"b\nc\\d".to_string(),
            }],
            value: MetricValue::Counter(1),
            timestamp_ms: 0,
        });

        let rendered = reg.render();
        assert!(rendered.contains("path=\"a\\\"b\\nc\\\\d\""));
    }
}
