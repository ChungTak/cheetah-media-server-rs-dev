//! WebRTC module operator metrics.
//!
//! Phase 04 §4.8 enumerated the metrics surface for the WebRTC
//! module. This module exposes a single aggregator —
//! [`WebRtcModuleMetrics`] — backed by atomic counters that the
//! driver event worker bumps in-band, plus a [`metrics_snapshot()`]
//! method on [`WebRtcModule`](crate::module::WebRtcModule) that
//! folds the live session registry and bridge state into the
//! returned [`WebRtcModuleMetricsSnapshot`].
//!
//! ## Why an aggregator instead of per-session telemetry
//!
//! `cheetah-webrtc-module::session::WebRtcSessionTelemetry` already
//! holds per-session counters surfaced via `GET /session/{id}`. A
//! Prometheus-style operator dashboard wants stream-level totals
//! (`webrtc_packets_in_total`, …) alongside lightweight gauges
//! (`webrtc_sessions_active`). Rolling those up from the per-session
//! telemetry on every scrape would lock the registry; instead the
//! aggregator keeps cumulative `AtomicU64` counters that are
//! incremented as events arrive, plus the registry is consulted
//! cheaply for the active/publish/play gauge.
//!
//! All counters are monotonic and `Ordering::Relaxed`. Snapshots
//! are cheap to take (a handful of atomic loads + a `parking_lot`
//! read of the registry's HashMap length).
//!
//! ## Documented metrics surface
//!
//! Per §4.8 (gauge fields are read from registry / bridge state on
//! snapshot; counter fields increment in-band):
//!
//! * `webrtc_sessions_active` (gauge) — total active sessions
//! * `webrtc_publish_sessions` (gauge) — sessions in `Publisher`
//!   role (WHIP / SMS publish / pull / P2P publish)
//! * `webrtc_play_sessions` (gauge) — sessions in `Player` role
//! * `webrtc_packets_in_total` (counter)
//! * `webrtc_packets_out_total` (counter)
//! * `webrtc_nack_in_total` / `webrtc_nack_out_total`
//! * `webrtc_rtx_sent_total` / `webrtc_rtx_miss_total`
//! * `webrtc_pli_total` / `webrtc_fir_total`
//! * `webrtc_remb_bitrate_bps` (gauge, last observed)
//! * `webrtc_twcc_feedback_total`
//! * `webrtc_bwe_estimate_bps` (gauge, last observed)
//! * `webrtc_simulcast_layer_switch_total`
//! * `webrtc_route_migration_total`
//! * `webrtc_queue_drop_total`
//!
//! §"Phase 02 follow-up 第十三轮：local candidate counters"
//!
//! * `webrtc_local_candidate_host_total` (counter)
//! * `webrtc_local_candidate_srflx_total` (counter)
//! * `webrtc_local_candidate_prflx_total` (counter)
//! * `webrtc_local_candidate_relay_total` (counter)
//! * `webrtc_local_candidate_udp_total` (counter)
//! * `webrtc_local_candidate_tcp_total` (counter)
//! * `webrtc_local_candidate_ipv4_total` (counter)
//! * `webrtc_local_candidate_ipv6_total` (counter)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cheetah_webrtc_driver_tokio::LocalCandidateCounts;

/// Cumulative counters owned by the module event worker.
///
/// All counters are `AtomicU64` + `Ordering::Relaxed`. The aggregator
/// is constructed once per module instance and shared (via `Arc`)
/// with the event-worker task. `Default::default()` initialises all
/// counters to zero.
#[derive(Debug, Default)]
pub struct WebRtcModuleMetrics {
    pub(crate) packets_in: AtomicU64,
    pub(crate) packets_out: AtomicU64,
    pub(crate) nack_in: AtomicU64,
    pub(crate) nack_out: AtomicU64,
    pub(crate) rtx_sent: AtomicU64,
    pub(crate) rtx_miss: AtomicU64,
    pub(crate) pli: AtomicU64,
    pub(crate) fir: AtomicU64,
    pub(crate) twcc_feedback: AtomicU64,
    pub(crate) simulcast_layer_switches: AtomicU64,
    pub(crate) route_migrations: AtomicU64,
    pub(crate) queue_drops: AtomicU64,
    /// Last observed REMB bitrate in bps. Stored atomically because
    /// the snapshot reader does not hold the registry lock; the
    /// last-writer-wins semantic matches the gauge contract.
    pub(crate) remb_bitrate_bps: AtomicU64,
    /// Last observed BWE estimate in bps.
    pub(crate) bwe_estimate_bps: AtomicU64,

    // Phase 02 follow-up: local candidate counters
    pub(crate) local_candidate_host: AtomicU64,
    pub(crate) local_candidate_srflx: AtomicU64,
    pub(crate) local_candidate_prflx: AtomicU64,
    pub(crate) local_candidate_relay: AtomicU64,
    pub(crate) local_candidate_udp: AtomicU64,
    pub(crate) local_candidate_tcp: AtomicU64,
    pub(crate) local_candidate_ipv4: AtomicU64,
    pub(crate) local_candidate_ipv6: AtomicU64,

    // Phase 04 task 4.2: play disconnect counters
    /// Total play disconnects that exceeded the minimum duration
    /// threshold and emitted a business event.
    pub(crate) play_disconnect_events: AtomicU64,
    /// Total short play connections (below threshold) that only
    /// recorded a metric without emitting a business event.
    pub(crate) play_disconnect_short: AtomicU64,
}

impl WebRtcModuleMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Add stat-event deltas to the aggregate counters.
    ///
    /// `cheetah-webrtc-core` emits stats as cumulative per-session
    /// snapshots. The module event worker computes the delta from
    /// the prior snapshot before calling this method, so the
    /// aggregator stays a strictly increasing counter even when
    /// individual sessions reset (e.g., on session close + new
    /// session reusing low ids).
    pub fn add_stats_delta(&self, delta: &WebRtcSessionStatsDelta) {
        self.packets_in
            .fetch_add(delta.packets_in, Ordering::Relaxed);
        self.packets_out
            .fetch_add(delta.packets_out, Ordering::Relaxed);
        self.nack_in.fetch_add(delta.nack_in, Ordering::Relaxed);
        self.nack_out.fetch_add(delta.nack_out, Ordering::Relaxed);
        self.rtx_sent.fetch_add(delta.rtx_sent, Ordering::Relaxed);
        self.rtx_miss.fetch_add(delta.rtx_miss, Ordering::Relaxed);
        self.pli.fetch_add(delta.pli, Ordering::Relaxed);
        self.fir.fetch_add(delta.fir, Ordering::Relaxed);
    }

    pub fn record_remb(&self, bps: u64) {
        self.remb_bitrate_bps.store(bps, Ordering::Relaxed);
    }

    pub fn record_bwe(&self, bps: u64) {
        self.bwe_estimate_bps.store(bps, Ordering::Relaxed);
    }

    pub fn inc_twcc_feedback(&self) {
        self.twcc_feedback.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_simulcast_layer_switch(&self) {
        self.simulcast_layer_switches
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_route_migration(&self) {
        self.route_migrations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_queue_drop(&self) {
        self.queue_drops.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a play disconnect that exceeded the minimum duration
    /// threshold and emitted a business event.
    pub fn inc_play_disconnect_event(&self) {
        self.play_disconnect_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a short play connection (below threshold) that only
    /// recorded a metric without emitting a business event.
    pub fn inc_play_disconnect_short(&self) {
        self.play_disconnect_short.fetch_add(1, Ordering::Relaxed);
    }

    /// Accumulate a local candidate snapshot into the running totals.
    ///
    /// Each `LocalCandidateSnapshot` event from the driver carries a
    /// [`LocalCandidateCounts`] describing the candidates gathered for
    /// one session. This method adds those counts to the module-wide
    /// monotonic counters, matching the packet counter pattern used
    /// elsewhere in this struct.
    pub fn record_local_candidate_snapshot(&self, counts: LocalCandidateCounts) {
        self.local_candidate_host
            .fetch_add(counts.host as u64, Ordering::Relaxed);
        self.local_candidate_srflx
            .fetch_add(counts.srflx as u64, Ordering::Relaxed);
        self.local_candidate_prflx
            .fetch_add(counts.prflx as u64, Ordering::Relaxed);
        self.local_candidate_relay
            .fetch_add(counts.relay as u64, Ordering::Relaxed);
        self.local_candidate_udp
            .fetch_add(counts.udp as u64, Ordering::Relaxed);
        self.local_candidate_tcp
            .fetch_add(counts.tcp as u64, Ordering::Relaxed);
        self.local_candidate_ipv4
            .fetch_add(counts.ipv4 as u64, Ordering::Relaxed);
        self.local_candidate_ipv6
            .fetch_add(counts.ipv6 as u64, Ordering::Relaxed);
    }

    /// Snapshot the cumulative counters. Caller stitches the
    /// session-count gauges from the registry separately.
    pub(crate) fn snapshot_counters(&self) -> WebRtcModuleCounterSnapshot {
        WebRtcModuleCounterSnapshot {
            packets_in: self.packets_in.load(Ordering::Relaxed),
            packets_out: self.packets_out.load(Ordering::Relaxed),
            nack_in: self.nack_in.load(Ordering::Relaxed),
            nack_out: self.nack_out.load(Ordering::Relaxed),
            rtx_sent: self.rtx_sent.load(Ordering::Relaxed),
            rtx_miss: self.rtx_miss.load(Ordering::Relaxed),
            pli: self.pli.load(Ordering::Relaxed),
            fir: self.fir.load(Ordering::Relaxed),
            twcc_feedback: self.twcc_feedback.load(Ordering::Relaxed),
            simulcast_layer_switches: self.simulcast_layer_switches.load(Ordering::Relaxed),
            route_migrations: self.route_migrations.load(Ordering::Relaxed),
            queue_drops: self.queue_drops.load(Ordering::Relaxed),
            remb_bitrate_bps: self.remb_bitrate_bps.load(Ordering::Relaxed),
            bwe_estimate_bps: self.bwe_estimate_bps.load(Ordering::Relaxed),
            // Phase 02 follow-up 第十三轮：local candidate counters
            local_candidate_host: self.local_candidate_host.load(Ordering::Relaxed),
            local_candidate_srflx: self.local_candidate_srflx.load(Ordering::Relaxed),
            local_candidate_prflx: self.local_candidate_prflx.load(Ordering::Relaxed),
            local_candidate_relay: self.local_candidate_relay.load(Ordering::Relaxed),
            local_candidate_udp: self.local_candidate_udp.load(Ordering::Relaxed),
            local_candidate_tcp: self.local_candidate_tcp.load(Ordering::Relaxed),
            local_candidate_ipv4: self.local_candidate_ipv4.load(Ordering::Relaxed),
            local_candidate_ipv6: self.local_candidate_ipv6.load(Ordering::Relaxed),
            // Phase 04 task 4.2: play disconnect counters
            play_disconnect_events: self.play_disconnect_events.load(Ordering::Relaxed),
            play_disconnect_short: self.play_disconnect_short.load(Ordering::Relaxed),
        }
    }
}

/// Delta computed by the event worker from two consecutive
/// `WebRtcCoreEvent::Stats` snapshots for the same session.
///
/// Negative or zero values are clamped to zero by the caller, so
/// every field below is safe to add into the global counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebRtcSessionStatsDelta {
    pub packets_in: u64,
    pub packets_out: u64,
    pub nack_in: u64,
    pub nack_out: u64,
    pub rtx_sent: u64,
    pub rtx_miss: u64,
    pub pli: u64,
    pub fir: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WebRtcModuleCounterSnapshot {
    pub packets_in: u64,
    pub packets_out: u64,
    pub nack_in: u64,
    pub nack_out: u64,
    pub rtx_sent: u64,
    pub rtx_miss: u64,
    pub pli: u64,
    pub fir: u64,
    pub twcc_feedback: u64,
    pub simulcast_layer_switches: u64,
    pub route_migrations: u64,
    pub queue_drops: u64,
    pub remb_bitrate_bps: u64,
    pub bwe_estimate_bps: u64,
    // Phase 02 follow-up 第十三轮：local candidate counters
    pub local_candidate_host: u64,
    pub local_candidate_srflx: u64,
    pub local_candidate_prflx: u64,
    pub local_candidate_relay: u64,
    pub local_candidate_udp: u64,
    pub local_candidate_tcp: u64,
    pub local_candidate_ipv4: u64,
    pub local_candidate_ipv6: u64,
    // Phase 04 task 4.2: play disconnect counters
    pub play_disconnect_events: u64,
    pub play_disconnect_short: u64,
}

/// Operator-facing snapshot. Fields mirror the documented metric
/// names in phase-04 §4.8 (sans the `webrtc_` prefix; the prefix is
/// added by the Prometheus exporter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcModuleMetricsSnapshot {
    /// Active session count (gauge, all roles).
    pub sessions_active: u64,
    /// Sessions whose role is `Publisher` (WHIP / SMS publish / pull /
    /// P2P publish).
    pub publish_sessions: u64,
    /// Sessions whose role is `Player` (WHEP / SMS play / push).
    pub play_sessions: u64,
    pub packets_in_total: u64,
    pub packets_out_total: u64,
    pub nack_in_total: u64,
    pub nack_out_total: u64,
    pub rtx_sent_total: u64,
    pub rtx_miss_total: u64,
    pub pli_total: u64,
    pub fir_total: u64,
    pub twcc_feedback_total: u64,
    pub simulcast_layer_switch_total: u64,
    pub route_migration_total: u64,
    pub queue_drop_total: u64,
    /// Last observed REMB bitrate (gauge).
    pub remb_bitrate_bps: u64,
    /// Last observed BWE estimate (gauge).
    pub bwe_estimate_bps: u64,
    // Phase 02 follow-up 第十三轮：local candidate counters
    pub local_candidate_host_total: u64,
    pub local_candidate_srflx_total: u64,
    pub local_candidate_prflx_total: u64,
    pub local_candidate_relay_total: u64,
    pub local_candidate_udp_total: u64,
    pub local_candidate_tcp_total: u64,
    pub local_candidate_ipv4_total: u64,
    pub local_candidate_ipv6_total: u64,
    // Phase 04 task 4.2: play disconnect counters
    pub play_disconnect_events_total: u64,
    pub play_disconnect_short_total: u64,
}

impl WebRtcModuleMetricsSnapshot {
    /// Combine an aggregator counter snapshot with live registry
    /// gauges into a single operator-facing record.
    pub(crate) fn assemble(
        counters: WebRtcModuleCounterSnapshot,
        sessions_active: usize,
        publish_sessions: usize,
        play_sessions: usize,
    ) -> Self {
        Self {
            sessions_active: sessions_active as u64,
            publish_sessions: publish_sessions as u64,
            play_sessions: play_sessions as u64,
            packets_in_total: counters.packets_in,
            packets_out_total: counters.packets_out,
            nack_in_total: counters.nack_in,
            nack_out_total: counters.nack_out,
            rtx_sent_total: counters.rtx_sent,
            rtx_miss_total: counters.rtx_miss,
            pli_total: counters.pli,
            fir_total: counters.fir,
            twcc_feedback_total: counters.twcc_feedback,
            simulcast_layer_switch_total: counters.simulcast_layer_switches,
            route_migration_total: counters.route_migrations,
            queue_drop_total: counters.queue_drops,
            remb_bitrate_bps: counters.remb_bitrate_bps,
            bwe_estimate_bps: counters.bwe_estimate_bps,
            // Phase 02 follow-up 第十三轮：local candidate counters
            local_candidate_host_total: counters.local_candidate_host,
            local_candidate_srflx_total: counters.local_candidate_srflx,
            local_candidate_prflx_total: counters.local_candidate_prflx,
            local_candidate_relay_total: counters.local_candidate_relay,
            local_candidate_udp_total: counters.local_candidate_udp,
            local_candidate_tcp_total: counters.local_candidate_tcp,
            local_candidate_ipv4_total: counters.local_candidate_ipv4,
            local_candidate_ipv6_total: counters.local_candidate_ipv6,
            // Phase 04 task 4.2: play disconnect counters
            play_disconnect_events_total: counters.play_disconnect_events,
            play_disconnect_short_total: counters.play_disconnect_short,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_default_is_all_zero() {
        let m = WebRtcModuleMetrics::new();
        let snap = m.snapshot_counters();
        assert_eq!(snap.packets_in, 0);
        assert_eq!(snap.nack_out, 0);
        assert_eq!(snap.remb_bitrate_bps, 0);
        assert_eq!(snap.bwe_estimate_bps, 0);
    }

    #[test]
    fn add_stats_delta_accumulates() {
        let m = WebRtcModuleMetrics::new();
        m.add_stats_delta(&WebRtcSessionStatsDelta {
            packets_in: 100,
            packets_out: 200,
            nack_in: 5,
            nack_out: 7,
            ..Default::default()
        });
        m.add_stats_delta(&WebRtcSessionStatsDelta {
            packets_in: 50,
            nack_out: 3,
            pli: 1,
            ..Default::default()
        });
        let snap = m.snapshot_counters();
        assert_eq!(snap.packets_in, 150);
        assert_eq!(snap.packets_out, 200);
        assert_eq!(snap.nack_in, 5);
        assert_eq!(snap.nack_out, 10);
        assert_eq!(snap.pli, 1);
    }

    #[test]
    fn record_remb_and_bwe_are_last_writer_wins() {
        let m = WebRtcModuleMetrics::new();
        m.record_remb(1_000_000);
        m.record_remb(500_000);
        m.record_bwe(2_000_000);
        let snap = m.snapshot_counters();
        assert_eq!(snap.remb_bitrate_bps, 500_000);
        assert_eq!(snap.bwe_estimate_bps, 2_000_000);
    }

    #[test]
    fn assemble_combines_counters_and_gauges() {
        let m = WebRtcModuleMetrics::new();
        m.add_stats_delta(&WebRtcSessionStatsDelta {
            packets_in: 10,
            ..Default::default()
        });
        m.inc_route_migration();
        m.inc_route_migration();
        m.record_bwe(1_500_000);
        let snap = WebRtcModuleMetricsSnapshot::assemble(m.snapshot_counters(), 7, 3, 4);
        assert_eq!(snap.sessions_active, 7);
        assert_eq!(snap.publish_sessions, 3);
        assert_eq!(snap.play_sessions, 4);
        assert_eq!(snap.packets_in_total, 10);
        assert_eq!(snap.route_migration_total, 2);
        assert_eq!(snap.bwe_estimate_bps, 1_500_000);
    }

    /// Phase 04 follow-up: TWCC feedback counter increments once per
    /// BWE event from str0m. The driver event worker calls
    /// `inc_twcc_feedback()` when a `Bwe` snapshot arrives, so over a
    /// stream of N BWE events the counter should equal N.
    #[test]
    fn twcc_feedback_counter_increments_per_bwe_event() {
        let m = WebRtcModuleMetrics::new();
        // Simulate 20 BWE events (str0m's TWCC trigger threshold is
        // 20 packets or 256ms).
        for _ in 0..20 {
            m.inc_twcc_feedback();
        }
        let snap = m.snapshot_counters();
        assert_eq!(snap.twcc_feedback, 20);
    }

    /// TWCC feedback counter is monotonic and never decrements.
    #[test]
    fn twcc_feedback_counter_is_monotonic() {
        let m = WebRtcModuleMetrics::new();
        m.inc_twcc_feedback();
        let after_one = m.snapshot_counters().twcc_feedback;
        m.inc_twcc_feedback();
        let after_two = m.snapshot_counters().twcc_feedback;
        assert!(after_two > after_one);
        assert_eq!(after_two, after_one + 1);
    }

    /// PLI/FIR/REMB/TWCC counters are independent — incrementing one
    /// does not affect the others.
    #[test]
    fn rtcp_counters_are_independent() {
        let m = WebRtcModuleMetrics::new();
        m.pli.fetch_add(5, std::sync::atomic::Ordering::Relaxed);
        m.fir.fetch_add(3, std::sync::atomic::Ordering::Relaxed);
        m.inc_twcc_feedback();
        m.record_remb(2_000_000);
        let snap = m.snapshot_counters();
        assert_eq!(snap.pli, 5);
        assert_eq!(snap.fir, 3);
        assert_eq!(snap.twcc_feedback, 1);
        assert_eq!(snap.remb_bitrate_bps, 2_000_000);
    }

    /// Phase 02 follow-up 第十三轮: calling `record_local_candidate_snapshot`
    /// multiple times accumulates each field independently.
    #[test]
    fn record_local_candidate_snapshot_accumulates_per_type() {
        let m = WebRtcModuleMetrics::new();
        m.record_local_candidate_snapshot(LocalCandidateCounts {
            host: 2,
            srflx: 1,
            prflx: 0,
            relay: 1,
            udp: 2,
            tcp: 1,
            ipv4: 2,
            ipv6: 1,
        });
        m.record_local_candidate_snapshot(LocalCandidateCounts {
            host: 3,
            srflx: 0,
            prflx: 1,
            relay: 2,
            udp: 1,
            tcp: 0,
            ipv4: 1,
            ipv6: 3,
        });
        let snap = m.snapshot_counters();
        assert_eq!(snap.local_candidate_host, 5);
        assert_eq!(snap.local_candidate_srflx, 1);
        assert_eq!(snap.local_candidate_prflx, 1);
        assert_eq!(snap.local_candidate_relay, 3);
        assert_eq!(snap.local_candidate_udp, 3);
        assert_eq!(snap.local_candidate_tcp, 1);
        assert_eq!(snap.local_candidate_ipv4, 3);
        assert_eq!(snap.local_candidate_ipv6, 4);
    }

    /// Phase 02 follow-up 第十三轮: local candidate counters are monotonic —
    /// a second snapshot always yields values >= the first.
    #[test]
    fn record_local_candidate_snapshot_is_monotonic() {
        let m = WebRtcModuleMetrics::new();
        m.record_local_candidate_snapshot(LocalCandidateCounts {
            host: 1,
            srflx: 2,
            prflx: 0,
            relay: 0,
            udp: 1,
            tcp: 1,
            ipv4: 1,
            ipv6: 1,
        });
        let first = m.snapshot_counters();
        m.record_local_candidate_snapshot(LocalCandidateCounts {
            host: 0,
            srflx: 0,
            prflx: 1,
            relay: 3,
            udp: 0,
            tcp: 0,
            ipv4: 0,
            ipv6: 0,
        });
        let second = m.snapshot_counters();
        assert!(second.local_candidate_host >= first.local_candidate_host);
        assert!(second.local_candidate_srflx >= first.local_candidate_srflx);
        assert!(second.local_candidate_prflx >= first.local_candidate_prflx);
        assert!(second.local_candidate_relay >= first.local_candidate_relay);
        assert!(second.local_candidate_udp >= first.local_candidate_udp);
        assert!(second.local_candidate_tcp >= first.local_candidate_tcp);
        assert!(second.local_candidate_ipv4 >= first.local_candidate_ipv4);
        assert!(second.local_candidate_ipv6 >= first.local_candidate_ipv6);
    }

    /// Phase 02 follow-up 第十三轮: a fresh `WebRtcModuleMetrics` has all
    /// 8 local candidate counters at zero.
    #[test]
    fn local_candidate_counters_default_to_zero() {
        let m = WebRtcModuleMetrics::new();
        let snap = m.snapshot_counters();
        assert_eq!(snap.local_candidate_host, 0);
        assert_eq!(snap.local_candidate_srflx, 0);
        assert_eq!(snap.local_candidate_prflx, 0);
        assert_eq!(snap.local_candidate_relay, 0);
        assert_eq!(snap.local_candidate_udp, 0);
        assert_eq!(snap.local_candidate_tcp, 0);
        assert_eq!(snap.local_candidate_ipv4, 0);
        assert_eq!(snap.local_candidate_ipv6, 0);
    }

    /// Phase 02 follow-up 第十三轮: `assemble()` propagates local candidate
    /// counters into the `WebRtcModuleMetricsSnapshot` `*_total` fields.
    #[test]
    fn assemble_includes_local_candidate_fields() {
        let m = WebRtcModuleMetrics::new();
        m.record_local_candidate_snapshot(LocalCandidateCounts {
            host: 4,
            srflx: 3,
            prflx: 2,
            relay: 1,
            udp: 5,
            tcp: 2,
            ipv4: 6,
            ipv6: 1,
        });
        let snap = WebRtcModuleMetricsSnapshot::assemble(m.snapshot_counters(), 1, 1, 0);
        assert_eq!(snap.local_candidate_host_total, 4);
        assert_eq!(snap.local_candidate_srflx_total, 3);
        assert_eq!(snap.local_candidate_prflx_total, 2);
        assert_eq!(snap.local_candidate_relay_total, 1);
        assert_eq!(snap.local_candidate_udp_total, 5);
        assert_eq!(snap.local_candidate_tcp_total, 2);
        assert_eq!(snap.local_candidate_ipv4_total, 6);
        assert_eq!(snap.local_candidate_ipv6_total, 1);
    }
}

#[cfg(test)]
mod module_integration_tests {
    use crate::WebRtcModule;

    /// `WebRtcModule::metrics_snapshot()` on a freshly-constructed
    /// (uninitialised) module returns the all-zero snapshot. This
    /// is the contract operator dashboards rely on at startup
    /// before any sessions exist.
    #[test]
    fn module_metrics_snapshot_starts_at_zero() {
        let module = WebRtcModule::new();
        let snap = module.metrics_snapshot();
        assert_eq!(snap.sessions_active, 0);
        assert_eq!(snap.publish_sessions, 0);
        assert_eq!(snap.play_sessions, 0);
        assert_eq!(snap.packets_in_total, 0);
        assert_eq!(snap.packets_out_total, 0);
        assert_eq!(snap.nack_in_total, 0);
        assert_eq!(snap.nack_out_total, 0);
        assert_eq!(snap.pli_total, 0);
        assert_eq!(snap.fir_total, 0);
        assert_eq!(snap.twcc_feedback_total, 0);
        assert_eq!(snap.simulcast_layer_switch_total, 0);
        assert_eq!(snap.route_migration_total, 0);
        assert_eq!(snap.queue_drop_total, 0);
        assert_eq!(snap.remb_bitrate_bps, 0);
        assert_eq!(snap.bwe_estimate_bps, 0);
        assert_eq!(snap.play_disconnect_events_total, 0);
        assert_eq!(snap.play_disconnect_short_total, 0);
    }
}
