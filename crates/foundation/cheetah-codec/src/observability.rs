//! Layer-aware observability baseline (`SystemArchitecture.md` §4).
//!
//! This module is Sans-I/O: it only classifies repair events and computes the
//! runtime-report schema from explicitly injected timing samples. Drivers and
//! modules feed it wall-clock (`now_us`) and canonical media timestamps; it
//! never reads the clock itself.
//!
//! Two concerns live here:
//!
//! * **Layer-aware repair accounting** — timestamp repairs are attributed to
//!   the `source` / `canonical` / `egress` timeline (see §4). Normal B-frame
//!   reorder noise stays on the source layer and must not escalate
//!   canonical/egress anomaly warnings; canonical/egress repairs are only
//!   flagged once they cross [`REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`].
//! * **Runtime report schema** — the baseline metrics
//!   (`startup_latency_ms`, `first_second_avg_frame_interval_ms`,
//!   `average_playback_rate_x`, `first_keyframe_delay_ms`) plus the three repair
//!   counters, computed by [`RuntimeReportBuilder`].

use crate::time::TimestampAlert;

/// Explicit high-frequency threshold used to decide whether canonical/egress
/// repair volume should escalate to an anomaly warning (see §4).
pub const REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD: u64 = 32;

/// The timeline layer a repair event is attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepairLayer {
    /// Source-timeline reorder/repair observations (including B-frame reorder
    /// noise). Never escalates canonical/egress warnings.
    Source,
    /// Canonical-timeline monotonic repair events.
    Canonical,
    /// Protocol-export monotonic repair events.
    Egress,
}

impl RepairLayer {
    /// Stable lowercase label used as a metric/attribute name.
    pub fn label(self) -> &'static str {
        match self {
            RepairLayer::Source => "source",
            RepairLayer::Canonical => "canonical",
            RepairLayer::Egress => "egress",
        }
    }
}

/// Classify a timestamp-normalizer alert into the repair layer it belongs to.
///
/// Pure discontinuity/reset markers (`TimelineDiscontinuityDetected`,
/// `ResetApplied`) are *not* repairs and return `None` so they are never
/// counted as repair events.
pub fn classify_timestamp_alert(alert: TimestampAlert) -> Option<RepairLayer> {
    match alert {
        // Source timeline reconstruction / reorder noise.
        TimestampAlert::PtsReorderObserved
        | TimestampAlert::MissingDtsUsedFallback
        | TimestampAlert::MissingPtsDerivedFromDts => Some(RepairLayer::Source),
        // Canonical timeline monotonic repair.
        TimestampAlert::NonMonotonicDtsRepaired | TimestampAlert::NegativeCompositionClamped => {
            Some(RepairLayer::Canonical)
        }
        // Not repairs: discontinuity/reset boundaries.
        TimestampAlert::TimelineDiscontinuityDetected | TimestampAlert::ResetApplied => None,
    }
}

/// Layer-classified repair counters (see §4 observability baseline).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RepairEventCounters {
    /// `source_repair_events` field of type `u64`.
    /// `source_repair_events` 字段，类型为 `u64`.
    pub source_repair_events: u64,
    /// `canonical_repair_events` field of type `u64`.
    /// `canonical_repair_events` 字段，类型为 `u64`.
    pub canonical_repair_events: u64,
    /// `egress_repair_events` field of type `u64`.
    /// `egress_repair_events` 字段，类型为 `u64`.
    pub egress_repair_events: u64,
}

impl RepairEventCounters {
    /// Count a repair on an explicit layer.
    pub fn record_layer(&mut self, layer: RepairLayer) {
        let slot = match layer {
            RepairLayer::Source => &mut self.source_repair_events,
            RepairLayer::Canonical => &mut self.canonical_repair_events,
            RepairLayer::Egress => &mut self.egress_repair_events,
        };
        *slot = slot.saturating_add(1);
    }

    /// Count a normalizer alert, attributing it to its layer. Non-repair alerts
    /// (discontinuity/reset) are ignored.
    pub fn record_alert(&mut self, alert: TimestampAlert) {
        if let Some(layer) = classify_timestamp_alert(alert) {
            self.record_layer(layer);
        }
    }

    /// Count an egress-layer monotonic repair (e.g. the `repaired` result of
    /// [`crate::repair_monotonic_timestamp`]).
    pub fn record_egress_repair(&mut self) {
        self.record_layer(RepairLayer::Egress);
    }

    /// Total count for a given layer.
    pub fn count(&self, layer: RepairLayer) -> u64 {
        match layer {
            RepairLayer::Source => self.source_repair_events,
            RepairLayer::Canonical => self.canonical_repair_events,
            RepairLayer::Egress => self.egress_repair_events,
        }
    }

    /// Whether the given layer's repair volume should escalate to a
    /// high-frequency anomaly warning.
    ///
    /// The source layer never escalates (B-frame reorder noise is expected);
    /// canonical/egress escalate only at/above
    /// [`REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD`].
    pub fn is_high_frequency_anomaly(&self, layer: RepairLayer) -> bool {
        match layer {
            RepairLayer::Source => false,
            RepairLayer::Canonical | RepairLayer::Egress => {
                self.count(layer) >= REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD
            }
        }
    }
}

/// Runtime observability report schema (see §4).
///
/// Each timing metric is optional because it is only defined once the relevant
/// event has been observed (first frame / first keyframe / a media span).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct RuntimeObservabilityReport {
    /// Wall-clock delay from session start to the first delivered frame.
    pub startup_latency_ms: Option<f64>,
    /// Average inter-frame wall-clock interval over the first second of output.
    pub first_second_avg_frame_interval_ms: Option<f64>,
    /// Observed playback speed: media time advanced / wall time elapsed.
    pub average_playback_rate_x: Option<f64>,
    /// Wall-clock delay from session start to the first keyframe.
    pub first_keyframe_delay_ms: Option<f64>,
    /// Layer-classified repair counters.
    pub repairs: RepairEventCounters,
}

/// Accumulates timing samples for a single session/stream and produces a
/// [`RuntimeObservabilityReport`].
///
/// Sans-I/O: the caller injects a monotonic wall clock (`now_us`) and canonical
/// media presentation timestamps (`pts_us`); this type performs no I/O and reads
/// no clock. Feed it via [`RuntimeReportBuilder::on_frame`] on the egress path
/// and via the `record_*` helpers when repairs are observed.
#[derive(Debug, Clone)]
pub struct RuntimeReportBuilder {
    /// `session_start_us` field of type `i64`.
    /// `session_start_us` 字段，类型为 `i64`.
    session_start_us: i64,
    /// `first_frame_wall_us` field.
    /// `first_frame_wall_us` 字段.
    first_frame_wall_us: Option<i64>,
    /// `last_frame_wall_us` field.
    /// `last_frame_wall_us` 字段.
    last_frame_wall_us: Option<i64>,
    /// `first_second_interval_sum_us` field of type `i64`.
    /// `first_second_interval_sum_us` 字段，类型为 `i64`.
    first_second_interval_sum_us: i64,
    /// `first_second_interval_count` field of type `u64`.
    /// `first_second_interval_count` 字段，类型为 `u64`.
    first_second_interval_count: u64,
    /// `first_keyframe_wall_us` field.
    /// `first_keyframe_wall_us` 字段.
    first_keyframe_wall_us: Option<i64>,
    /// `first_pts_us` field.
    /// `first_pts_us` 字段.
    first_pts_us: Option<i64>,
    /// `first_pts_wall_us` field.
    /// `first_pts_wall_us` 字段.
    first_pts_wall_us: Option<i64>,
    /// `last_pts_us` field.
    /// `last_pts_us` 字段.
    last_pts_us: Option<i64>,
    /// `last_pts_wall_us` field.
    /// `last_pts_wall_us` 字段.
    last_pts_wall_us: Option<i64>,
    /// `repairs` field of type `RepairEventCounters`.
    /// `repairs` 字段，类型为 `RepairEventCounters`.
    repairs: RepairEventCounters,
}

impl RuntimeReportBuilder {
    /// Start a report for a session that began at `session_start_us` (monotonic
    /// wall clock, microseconds).
    pub fn new(session_start_us: i64) -> Self {
        Self {
            session_start_us,
            first_frame_wall_us: None,
            last_frame_wall_us: None,
            first_second_interval_sum_us: 0,
            first_second_interval_count: 0,
            first_keyframe_wall_us: None,
            first_pts_us: None,
            first_pts_wall_us: None,
            last_pts_us: None,
            last_pts_wall_us: None,
            repairs: RepairEventCounters::default(),
        }
    }

    /// Record an egress frame delivered at wall-clock `now_us` carrying canonical
    /// presentation timestamp `pts_us`.
    pub fn on_frame(&mut self, now_us: i64, pts_us: i64, is_keyframe: bool) {
        match self.first_frame_wall_us {
            None => {
                self.first_frame_wall_us = Some(now_us);
            }
            Some(first) => {
                if let Some(last) = self.last_frame_wall_us {
                    // Only accumulate intervals within the first second of output.
                    if now_us.saturating_sub(first) <= 1_000_000 {
                        self.first_second_interval_sum_us += now_us.saturating_sub(last);
                        self.first_second_interval_count += 1;
                    }
                }
            }
        }
        self.last_frame_wall_us = Some(now_us);

        if is_keyframe && self.first_keyframe_wall_us.is_none() {
            self.first_keyframe_wall_us = Some(now_us);
        }

        if self.first_pts_us.is_none() {
            self.first_pts_us = Some(pts_us);
            self.first_pts_wall_us = Some(now_us);
        }
        self.last_pts_us = Some(pts_us);
        self.last_pts_wall_us = Some(now_us);
    }

    /// Record a repair on an explicit layer.
    pub fn record_repair(&mut self, layer: RepairLayer) {
        self.repairs.record_layer(layer);
    }

    /// Record a normalizer alert (attributed to its layer, non-repairs ignored).
    pub fn record_alert(&mut self, alert: TimestampAlert) {
        self.repairs.record_alert(alert);
    }

    /// Current repair counters.
    pub fn repairs(&self) -> RepairEventCounters {
        self.repairs
    }

    /// Produce the report from the samples accumulated so far.
    pub fn build(&self) -> RuntimeObservabilityReport {
        let startup_latency_ms = self
            .first_frame_wall_us
            .map(|first| us_to_ms(first.saturating_sub(self.session_start_us)));

        let first_keyframe_delay_ms = self
            .first_keyframe_wall_us
            .map(|kf| us_to_ms(kf.saturating_sub(self.session_start_us)));

        let first_second_avg_frame_interval_ms = if self.first_second_interval_count > 0 {
            Some(
                us_to_ms(self.first_second_interval_sum_us)
                    / self.first_second_interval_count as f64,
            )
        } else {
            None
        };

        let average_playback_rate_x = match (
            self.first_pts_us,
            self.first_pts_wall_us,
            self.last_pts_us,
            self.last_pts_wall_us,
        ) {
            (Some(fp), Some(fw), Some(lp), Some(lw)) => {
                let media_span = lp.saturating_sub(fp);
                let wall_span = lw.saturating_sub(fw);
                if wall_span > 0 {
                    Some(media_span as f64 / wall_span as f64)
                } else {
                    None
                }
            }
            _ => None,
        };

        RuntimeObservabilityReport {
            startup_latency_ms,
            first_second_avg_frame_interval_ms,
            average_playback_rate_x,
            first_keyframe_delay_ms,
            repairs: self.repairs,
        }
    }
}

fn us_to_ms(us: i64) -> f64 {
    us as f64 / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn alert_strategy() -> impl Strategy<Value = TimestampAlert> {
        prop_oneof![
            Just(TimestampAlert::MissingDtsUsedFallback),
            Just(TimestampAlert::MissingPtsDerivedFromDts),
            Just(TimestampAlert::NonMonotonicDtsRepaired),
            Just(TimestampAlert::PtsReorderObserved),
            Just(TimestampAlert::TimelineDiscontinuityDetected),
            Just(TimestampAlert::NegativeCompositionClamped),
            Just(TimestampAlert::ResetApplied),
        ]
    }

    proptest! {
        /// No matter the alert mix, source-layer repairs never raise a
        /// high-frequency anomaly, and the per-layer totals equal the number of
        /// alerts classified into that layer (non-repair alerts are dropped).
        #[test]
        fn prop_layer_totals_and_source_never_escalates(
            alerts in proptest::collection::vec(alert_strategy(), 0..500),
        ) {
            let mut counters = RepairEventCounters::default();
            let (mut src, mut can, mut egr) = (0u64, 0u64, 0u64);
            for &a in &alerts {
                counters.record_alert(a);
                match classify_timestamp_alert(a) {
                    Some(RepairLayer::Source) => src += 1,
                    Some(RepairLayer::Canonical) => can += 1,
                    Some(RepairLayer::Egress) => egr += 1,
                    None => {}
                }
            }
            prop_assert_eq!(counters.source_repair_events, src);
            prop_assert_eq!(counters.canonical_repair_events, can);
            prop_assert_eq!(counters.egress_repair_events, egr);
            // Source noise must never escalate.
            prop_assert!(!counters.is_high_frequency_anomaly(RepairLayer::Source));
        }

        /// Canonical/egress anomaly triggers iff the layer count reaches the
        /// explicit threshold.
        #[test]
        fn prop_canonical_egress_threshold(
            canonical in 0u64..(REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD * 2),
            egress in 0u64..(REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD * 2),
        ) {
            let counters = RepairEventCounters {
                source_repair_events: 0,
                canonical_repair_events: canonical,
                egress_repair_events: egress,
            };
            prop_assert_eq!(
                counters.is_high_frequency_anomaly(RepairLayer::Canonical),
                canonical >= REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD
            );
            prop_assert_eq!(
                counters.is_high_frequency_anomaly(RepairLayer::Egress),
                egress >= REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD
            );
        }

        /// Startup latency and first-keyframe delay are non-negative and the
        /// keyframe never precedes the first frame, for any monotonic frame
        /// arrival schedule.
        #[test]
        fn prop_report_latencies_non_negative(
            start in 0i64..1_000_000,
            gaps in proptest::collection::vec(1i64..50_000, 1..40),
            keyframe_index in 0usize..40,
        ) {
            let mut builder = RuntimeReportBuilder::new(start);
            let mut now = start + 1;
            let mut pts = 0i64;
            for (i, gap) in gaps.iter().enumerate() {
                let is_kf = i == keyframe_index.min(gaps.len().saturating_sub(1));
                builder.on_frame(now, pts, is_kf);
                now += *gap;
                pts += *gap;
            }
            let report = builder.build();
            let startup = report.startup_latency_ms.expect("frame delivered");
            prop_assert!(startup >= 0.0);
            if let Some(kf) = report.first_keyframe_delay_ms {
                prop_assert!(kf >= 0.0);
                prop_assert!(kf >= startup - 1e-9);
            }
            if let Some(rate) = report.average_playback_rate_x {
                prop_assert!(rate >= 0.0);
            }
        }
    }

    #[test]
    fn source_alerts_never_escalate() {
        let mut counters = RepairEventCounters::default();
        for _ in 0..(REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD * 4) {
            counters.record_alert(TimestampAlert::PtsReorderObserved);
        }
        assert_eq!(
            counters.source_repair_events,
            REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD * 4
        );
        assert_eq!(counters.canonical_repair_events, 0);
        assert_eq!(counters.egress_repair_events, 0);
        assert!(!counters.is_high_frequency_anomaly(RepairLayer::Source));
    }

    #[test]
    fn canonical_escalates_at_threshold() {
        let mut counters = RepairEventCounters::default();
        for _ in 0..(REPAIR_WARN_HIGH_FREQUENCY_THRESHOLD - 1) {
            counters.record_alert(TimestampAlert::NonMonotonicDtsRepaired);
        }
        assert!(!counters.is_high_frequency_anomaly(RepairLayer::Canonical));
        counters.record_alert(TimestampAlert::NonMonotonicDtsRepaired);
        assert!(counters.is_high_frequency_anomaly(RepairLayer::Canonical));
    }

    #[test]
    fn discontinuity_and_reset_are_not_repairs() {
        assert_eq!(
            classify_timestamp_alert(TimestampAlert::TimelineDiscontinuityDetected),
            None
        );
        assert_eq!(classify_timestamp_alert(TimestampAlert::ResetApplied), None);
    }

    #[test]
    fn runtime_report_computes_baseline_metrics() {
        let mut builder = RuntimeReportBuilder::new(1_000_000);
        // First frame 20ms after start, non-keyframe absent -> keyframe at 20ms.
        builder.on_frame(1_020_000, 0, true);
        builder.on_frame(1_040_000, 33_000, false);
        builder.on_frame(1_060_000, 66_000, false);
        let report = builder.build();

        assert_eq!(report.startup_latency_ms, Some(20.0));
        assert_eq!(report.first_keyframe_delay_ms, Some(20.0));
        // Two intervals of 20ms each.
        assert_eq!(report.first_second_avg_frame_interval_ms, Some(20.0));
        // media advanced 66ms over 40ms wall -> 1.65x.
        let rate = report.average_playback_rate_x.expect("rate");
        assert!((rate - 1.65).abs() < 1e-6, "rate={rate}");
    }

    #[test]
    fn runtime_report_is_empty_before_first_frame() {
        let report = RuntimeReportBuilder::new(0).build();
        assert_eq!(report.startup_latency_ms, None);
        assert_eq!(report.first_keyframe_delay_ms, None);
        assert_eq!(report.first_second_avg_frame_interval_ms, None);
        assert_eq!(report.average_playback_rate_x, None);
    }
}
