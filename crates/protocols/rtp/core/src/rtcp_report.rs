//! RTCP report generation from per-session RTP state.
//!
//! RTCP-02 实现：从 RTP 状态生成 fraction lost、cumulative loss、highest seq、
//! jitter、LSR/DLSR，并组装成 `RtcpSenderReport` / `RtcpReceiverReport`。

use crate::rtcp::{RtcpReceiverReport, RtcpReportBlock, RtcpSenderReport};
use cheetah_codec::{RtpPayloadMode, RtpSequenceUnwrapper};

/// NTP epoch (1900-01-01) in seconds since Unix epoch (1970-01-01).
const NTP_UNIX_EPOCH_DIFF: u64 = 2_208_988_800;
const MS_PER_SEC: u64 = 1_000;
const FRACTION_PER_MS: u64 = (1u64 << 32) / 1_000;
const SEQUENCE_PERIOD: u64 = 1_u64 << 16;

/// Default RTP clock rate used for jitter/time conversions when no explicit
/// rate has been configured for the session.
pub fn default_clock_rate_hz(payload_mode: RtpPayloadMode) -> u64 {
    match payload_mode {
        RtpPayloadMode::RawAudio => 8_000,
        _ => 90_000,
    }
}

/// Convert wall-clock milliseconds to a 64-bit NTP timestamp.
pub fn ms_to_ntp_timestamp(now_ms: u64) -> u64 {
    let seconds = now_ms / MS_PER_SEC + NTP_UNIX_EPOCH_DIFF;
    let fraction = (now_ms % MS_PER_SEC) * FRACTION_PER_MS;
    (seconds << 32) | fraction
}

fn ntp_middle_32(ntp: u64) -> u32 {
    ((ntp >> 16) & 0xffff_ffff) as u32
}

/// Per-session RTCP report state.
#[derive(Debug, Clone)]
pub struct RtcpReportState {
    seq: RtpSequenceUnwrapper,
    base_seq: Option<u64>,
    expected_prior: u64,
    received_prior: u64,
    packets_received: u64,
    /// Full 64-bit NTP timestamp from the last received SR.
    last_sr_ntp: u64,
    /// `now_ms` when the last SR was received.
    last_sr_received_ms: u64,
    /// Interarrival jitter in RTP timestamp units.
    jitter: u64,
    last_transit: Option<i64>,
    /// RTP clock rate used to convert arrival time to RTP timestamp units.
    clock_rate_hz: u64,
    /// Last RTP timestamp that the local sender placed on an outbound packet.
    last_sent_rtp_timestamp: u32,
}

impl Default for RtcpReportState {
    fn default() -> Self {
        Self::new(90_000)
    }
}

impl RtcpReportState {
    /// Create a new state with the given RTP clock rate (Hz).
    pub fn new(clock_rate_hz: u64) -> Self {
        Self {
            seq: RtpSequenceUnwrapper::new(),
            base_seq: None,
            expected_prior: 0,
            received_prior: 0,
            packets_received: 0,
            last_sr_ntp: 0,
            last_sr_received_ms: 0,
            jitter: 0,
            last_transit: None,
            clock_rate_hz,
            last_sent_rtp_timestamp: 0,
        }
    }

    /// Set the clock rate. Usually updated when the codec or payload type is known.
    pub fn set_clock_rate(&mut self, hz: u64) {
        self.clock_rate_hz = hz;
    }

    /// Register an outbound RTP packet timestamp for the sender report.
    pub fn on_sent(&mut self, rtp_timestamp: u32) {
        self.last_sent_rtp_timestamp = rtp_timestamp;
    }

    /// Register an incoming RTP packet for statistics.
    pub fn on_packet(&mut self, seq: u16, rtp_timestamp: u32, now_ms: u64) {
        let extended = self.seq.extend(seq);
        if self.base_seq.is_none() {
            self.base_seq = Some(extended);
        }
        self.packets_received += 1;

        if self.clock_rate_hz == 0 {
            return;
        }

        let arrival_rtp_ts = (now_ms * self.clock_rate_hz) / MS_PER_SEC;
        let transit = i64::try_from(arrival_rtp_ts)
            .unwrap_or(i64::MAX)
            .wrapping_sub(i64::from(rtp_timestamp as i32));

        if let Some(prev) = self.last_transit {
            let d = transit.wrapping_sub(prev);
            let abs_d = d.unsigned_abs();
            // RFC 3550 A.8: J = J + (|D| - J) / 16
            self.jitter = self.jitter + (abs_d.saturating_sub(self.jitter)) / 16;
        }
        self.last_transit = Some(transit);
    }

    /// Register a received sender report so RR can echo LSR/DLSR.
    pub fn on_sender_report(&mut self, ntp: u64, now_ms: u64) {
        self.last_sr_ntp = ntp;
        self.last_sr_received_ms = now_ms;
    }

    /// Build an `RtcpReportBlock` for the current interval.
    pub fn report_block(&mut self, peer_ssrc: u32, now_ms: u64) -> Option<RtcpReportBlock> {
        let max_seq = self.seq.max_seq()?;
        let base = self.base_seq.unwrap_or(max_seq);
        let expected = max_seq.saturating_sub(base) + 1;
        let received = self.packets_received;

        let expected_interval = expected.saturating_sub(self.expected_prior);
        let received_interval = received.saturating_sub(self.received_prior);
        let lost_interval = expected_interval.saturating_sub(received_interval);

        let fraction_lost = if expected_interval > 0 {
            ((lost_interval * 256) / expected_interval).min(255) as u8
        } else {
            0
        };

        let cumulative =
            (expected as i64 - received as i64).clamp(-(1 << 23), (1 << 23) - 1) as i32;

        let cycles = (max_seq / SEQUENCE_PERIOD) as u32;
        let highest_seq16 = (max_seq % SEQUENCE_PERIOD) as u16;
        let highest_seq = (cycles << 16) | u32::from(highest_seq16);

        let last_sr = ntp_middle_32(self.last_sr_ntp);
        let delay_since_last_sr = if self.last_sr_received_ms > 0 {
            let delay_ms = now_ms.saturating_sub(self.last_sr_received_ms);
            (delay_ms * 65_536 / 1_000).min(u32::MAX as u64) as u32
        } else {
            0
        };

        self.expected_prior = expected;
        self.received_prior = received;

        Some(RtcpReportBlock {
            ssrc: peer_ssrc,
            fraction_lost,
            cumulative_lost: cumulative,
            highest_seq,
            jitter: self.jitter as u32,
            last_sr,
            delay_since_last_sr,
        })
    }

    /// Build an `RtcpSenderReport` for a sending session.
    pub fn sender_report(
        &self,
        ssrc: u32,
        packets_sent: u32,
        octets_sent: u32,
        now_ms: u64,
        report_block: Option<RtcpReportBlock>,
    ) -> RtcpSenderReport {
        RtcpSenderReport {
            ssrc,
            ntp_timestamp: ms_to_ntp_timestamp(now_ms),
            rtp_timestamp: self.last_sent_rtp_timestamp,
            packets_sent,
            octets_sent,
            report_blocks: report_block.into_iter().collect(),
        }
    }

    /// Build an `RtcpReceiverReport` for a receiving session.
    pub fn receiver_report(&self, ssrc: u32, report_block: RtcpReportBlock) -> RtcpReceiverReport {
        RtcpReceiverReport {
            ssrc,
            report_blocks: vec![report_block],
        }
    }

    /// Number of RTP packets that have contributed to the statistics.
    pub fn packets_received(&self) -> u64 {
        self.packets_received
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_block_reflects_lost_and_highest_seq() {
        let mut state = RtcpReportState::new(90_000);
        state.on_packet(0, 0, 0);
        state.on_packet(1, 90_000, 11);
        state.on_packet(3, 180_000, 22);

        let block = state.report_block(0x22222222, 22).unwrap();
        assert_eq!(block.ssrc, 0x22222222);
        assert_eq!(block.highest_seq, 3);
        assert_eq!(block.cumulative_lost, 1);
        assert_eq!(block.fraction_lost, 64);
    }

    #[test]
    fn jitter_estimates_interarrival_difference() {
        let mut state = RtcpReportState::new(90_000);
        state.on_packet(0, 0, 0);
        state.on_packet(1, 900, 10);
        state.on_packet(2, 1800, 20);
        let block = state.report_block(0, 30).unwrap();
        assert_eq!(block.jitter, 0);

        let mut state = RtcpReportState::new(90_000);
        state.on_packet(0, 0, 0);
        state.on_packet(1, 900, 40);
        let block = state.report_block(0, 40).unwrap();
        assert!(block.jitter > 0);
    }

    #[test]
    fn lsr_and_dlsr_echo_received_sender_report() {
        let mut state = RtcpReportState::new(90_000);
        let ntp = ms_to_ntp_timestamp(1000);
        state.on_sender_report(ntp, 1000);

        state.on_packet(0, 0, 1000);
        let block = state.report_block(0x22222222, 1500).unwrap();
        assert_eq!(block.last_sr, ntp_middle_32(ntp));
        assert_eq!(block.delay_since_last_sr, 32_768);
    }

    #[test]
    fn highest_seq_uses_cycle_count() {
        let mut state = RtcpReportState::new(90_000);
        state.on_packet(u16::MAX, 0, 0);
        state.on_packet(1, 90_000, 10);

        let block = state.report_block(0, 10).unwrap();
        // cycle 1, seq 1 => 0x00010001
        assert_eq!(block.highest_seq, 0x0001_0001);
    }
}
