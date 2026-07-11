use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cheetah_srt_core::SrtStreamMode;
use cheetah_srt_driver_tokio::SrtDriverStats;
use serde::Serialize;

#[derive(Debug, Default)]
/// Thread-safe atomic counters/gauges for SRT module telemetry.
///
/// SRT 模块遥测的线程安全原子计数器/仪表盘。
pub struct SrtModuleMetrics {
    connections_active: AtomicU64,
    connections_total: AtomicU64,
    publish_connections_total: AtomicU64,
    play_connections_total: AtomicU64,
    bytes_in_total: AtomicU64,
    bytes_out_total: AtomicU64,
    packets_in_total: AtomicU64,
    packets_out_total: AtomicU64,
    retransmit_total: AtomicU64,
    receiver_lost_total: AtomicU64,
    receiver_duplicate_total: AtomicU64,
    send_queue_depth: AtomicU64,
    recv_queue_depth: AtomicU64,
    rtt_micros: AtomicU64,
    jitter_micros: AtomicU64,
    key_refresh_total: AtomicU64,
    disconnect_total: AtomicU64,
    driver_errors_total: AtomicU64,
    send_queue_full_total: AtomicU64,
    handshake_reject_total: AtomicU64,
    handshake_reject_reasons: Mutex<BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
/// Read-only snapshot of `SrtModuleMetrics` for rendering and serialization.
///
/// 用于渲染与序列化的 `SrtModuleMetrics` 只读快照。
pub struct SrtModuleMetricsSnapshot {
    pub connections_active: u64,
    pub connections_total: u64,
    pub publish_connections_total: u64,
    pub play_connections_total: u64,
    pub bytes_in_total: u64,
    pub bytes_out_total: u64,
    pub packets_in_total: u64,
    pub packets_out_total: u64,
    pub retransmit_total: u64,
    pub receiver_lost_total: u64,
    pub receiver_duplicate_total: u64,
    pub send_queue_depth: u64,
    pub recv_queue_depth: u64,
    pub rtt_micros: u64,
    pub jitter_micros: u64,
    pub key_refresh_total: u64,
    pub disconnect_total: u64,
    pub driver_errors_total: u64,
    pub send_queue_full_total: u64,
    pub handshake_reject_total: u64,
    pub handshake_reject_reasons: BTreeMap<String, u64>,
}

/// `SrtModuleMetrics` API: increment, aggregate, and snapshot.
///
/// `SrtModuleMetrics` API：自增、聚合与快照。
impl SrtModuleMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn inc_connection(&self, mode: SrtStreamMode) {
        self.connections_active.fetch_add(1, Ordering::Relaxed);
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        match mode {
            SrtStreamMode::Publish => {
                self.publish_connections_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            SrtStreamMode::Request | SrtStreamMode::Play => {
                self.play_connections_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn dec_connection(&self) {
        decrement_saturating(&self.connections_active);
        self.disconnect_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_key_refresh(&self) {
        self.key_refresh_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_driver_error(&self, message: &str) {
        self.driver_errors_total.fetch_add(1, Ordering::Relaxed);
        if message == "SRT send queue full" {
            self.send_queue_full_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn inc_handshake_reject(&self, reason: &str) {
        self.handshake_reject_total.fetch_add(1, Ordering::Relaxed);
        let key = handshake_reject_reason_key(reason).to_string();
        if let Ok(mut reasons) = self.handshake_reject_reasons.lock() {
            *reasons.entry(key).or_default() += 1;
        }
    }

    pub fn add_stats_delta(&self, previous: Option<&SrtDriverStats>, current: &SrtDriverStats) {
        let baseline = previous.cloned().unwrap_or_default();
        self.bytes_in_total.fetch_add(
            current.bytes_in.saturating_sub(baseline.bytes_in),
            Ordering::Relaxed,
        );
        self.bytes_out_total.fetch_add(
            current.bytes_out.saturating_sub(baseline.bytes_out),
            Ordering::Relaxed,
        );
        self.packets_in_total.fetch_add(
            current.packets_in.saturating_sub(baseline.packets_in),
            Ordering::Relaxed,
        );
        self.packets_out_total.fetch_add(
            current.packets_out.saturating_sub(baseline.packets_out),
            Ordering::Relaxed,
        );
        self.retransmit_total.fetch_add(
            u64::from(
                current
                    .sender_total_retransmits
                    .saturating_sub(baseline.sender_total_retransmits),
            ),
            Ordering::Relaxed,
        );
        self.receiver_lost_total.fetch_add(
            current
                .receiver_total_lost
                .saturating_sub(baseline.receiver_total_lost),
            Ordering::Relaxed,
        );
        self.receiver_duplicate_total.fetch_add(
            current
                .receiver_total_duplicates
                .saturating_sub(baseline.receiver_total_duplicates),
            Ordering::Relaxed,
        );
        self.send_queue_depth.store(
            u64::from(current.sender_packets_in_buffer),
            Ordering::Relaxed,
        );
        self.recv_queue_depth.store(
            u64::from(current.receiver_packets_in_buffer),
            Ordering::Relaxed,
        );
        self.rtt_micros
            .store(u64::from(current.receiver_rtt_micros), Ordering::Relaxed);
        self.jitter_micros
            .store(u64::from(current.receiver_jitter_micros), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> SrtModuleMetricsSnapshot {
        let handshake_reject_reasons = self
            .handshake_reject_reasons
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default();
        SrtModuleMetricsSnapshot {
            connections_active: self.connections_active.load(Ordering::Relaxed),
            connections_total: self.connections_total.load(Ordering::Relaxed),
            publish_connections_total: self.publish_connections_total.load(Ordering::Relaxed),
            play_connections_total: self.play_connections_total.load(Ordering::Relaxed),
            bytes_in_total: self.bytes_in_total.load(Ordering::Relaxed),
            bytes_out_total: self.bytes_out_total.load(Ordering::Relaxed),
            packets_in_total: self.packets_in_total.load(Ordering::Relaxed),
            packets_out_total: self.packets_out_total.load(Ordering::Relaxed),
            retransmit_total: self.retransmit_total.load(Ordering::Relaxed),
            receiver_lost_total: self.receiver_lost_total.load(Ordering::Relaxed),
            receiver_duplicate_total: self.receiver_duplicate_total.load(Ordering::Relaxed),
            send_queue_depth: self.send_queue_depth.load(Ordering::Relaxed),
            recv_queue_depth: self.recv_queue_depth.load(Ordering::Relaxed),
            rtt_micros: self.rtt_micros.load(Ordering::Relaxed),
            jitter_micros: self.jitter_micros.load(Ordering::Relaxed),
            key_refresh_total: self.key_refresh_total.load(Ordering::Relaxed),
            disconnect_total: self.disconnect_total.load(Ordering::Relaxed),
            driver_errors_total: self.driver_errors_total.load(Ordering::Relaxed),
            send_queue_full_total: self.send_queue_full_total.load(Ordering::Relaxed),
            handshake_reject_total: self.handshake_reject_total.load(Ordering::Relaxed),
            handshake_reject_reasons,
        }
    }
}

/// Extract the `reject:` reason key from a close reason string.
///
/// 从关闭原因字符串中提取 `reject:` 原因键。
fn handshake_reject_reason_key(reason: &str) -> &str {
    let Some(rest) = reason.strip_prefix("reject:") else {
        return "unknown";
    };
    if rest.is_empty() {
        return "unknown";
    }
    rest.split_once(':').map(|(key, _)| key).unwrap_or(rest)
}

/// Saturating decrement for atomic gauges.
///
/// 原子仪表的饱和递减。
fn decrement_saturating(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        current.checked_sub(1)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_delta_is_monotonic() {
        let metrics = SrtModuleMetrics::new();
        let first = SrtDriverStats {
            bytes_in: 10,
            bytes_out: 20,
            packets_in: 1,
            packets_out: 2,
            sender_total_retransmits: 1,
            receiver_total_lost: 2,
            receiver_total_duplicates: 3,
            sender_packets_in_buffer: 4,
            receiver_packets_in_buffer: 5,
            receiver_rtt_micros: 6,
            receiver_jitter_micros: 7,
            ..Default::default()
        };
        let second = SrtDriverStats {
            bytes_in: 15,
            bytes_out: 25,
            packets_in: 3,
            packets_out: 4,
            sender_total_retransmits: 4,
            receiver_total_lost: 6,
            receiver_total_duplicates: 8,
            sender_packets_in_buffer: 9,
            receiver_packets_in_buffer: 10,
            receiver_rtt_micros: 11,
            receiver_jitter_micros: 12,
            ..Default::default()
        };

        metrics.add_stats_delta(None, &first);
        metrics.add_stats_delta(Some(&first), &second);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.bytes_in_total, 15);
        assert_eq!(snapshot.bytes_out_total, 25);
        assert_eq!(snapshot.packets_in_total, 3);
        assert_eq!(snapshot.packets_out_total, 4);
        assert_eq!(snapshot.retransmit_total, 4);
        assert_eq!(snapshot.receiver_lost_total, 6);
        assert_eq!(snapshot.receiver_duplicate_total, 8);
        assert_eq!(snapshot.send_queue_depth, 9);
        assert_eq!(snapshot.recv_queue_depth, 10);
        assert_eq!(snapshot.rtt_micros, 11);
        assert_eq!(snapshot.jitter_micros, 12);
    }

    #[test]
    fn connection_gauge_saturates_on_extra_disconnect() {
        let metrics = SrtModuleMetrics::new();

        metrics.inc_connection(SrtStreamMode::Publish);
        metrics.inc_connection(SrtStreamMode::Play);
        metrics.dec_connection();
        metrics.dec_connection();
        metrics.dec_connection();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.connections_active, 0);
        assert_eq!(snapshot.connections_total, 2);
        assert_eq!(snapshot.publish_connections_total, 1);
        assert_eq!(snapshot.play_connections_total, 1);
        assert_eq!(snapshot.disconnect_total, 3);
    }

    #[test]
    fn send_queue_full_error_has_specific_counter() {
        let metrics = SrtModuleMetrics::new();

        metrics.inc_driver_error("SRT send queue full");
        metrics.inc_driver_error("other driver error");

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.driver_errors_total, 2);
        assert_eq!(snapshot.send_queue_full_total, 1);
    }

    #[test]
    fn handshake_reject_tracks_reasons() {
        let metrics = SrtModuleMetrics::new();
        metrics.inc_handshake_reject("reject:invalid_stream_id: missing r");
        metrics.inc_handshake_reject("reject:auth_rejected");
        metrics.inc_handshake_reject("reject:publish_conflict: foo");

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.handshake_reject_total, 3);
        assert_eq!(snapshot.handshake_reject_reasons.get("invalid_stream_id"), Some(&1));
        assert_eq!(snapshot.handshake_reject_reasons.get("auth_rejected"), Some(&1));
        assert_eq!(snapshot.handshake_reject_reasons.get("publish_conflict"), Some(&1));
    }
}
