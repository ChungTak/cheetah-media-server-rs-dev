//! RTCP report generation from per-session RTP state.
//!
//! RTCP-02/03 实现：从 RTP 状态生成 fraction lost、cumulative loss、highest seq、
//! jitter、LSR/DLSR，并组装成 `RtcpSenderReport` / `RtcpReceiverReport`；
//! 同时维护 SR 中的 NTP/RTP 映射，将任意 RTP 时间戳映射到 NTP 与媒体时间线（微秒）。

use crate::rtcp::{RtcpReceiverReport, RtcpReportBlock, RtcpSenderReport};
use cheetah_codec::{RtpPayloadMode, RtpSequenceUnwrapper, WrapUnwrapper};

/// NTP epoch (1900-01-01) in seconds since Unix epoch (1970-01-01).
const NTP_UNIX_EPOCH_DIFF: u64 = 2_208_988_800;
const MS_PER_SEC: u64 = 1_000;
const FRACTION_PER_MS: u64 = (1u64 << 32) / 1_000;
const SEQUENCE_PERIOD: u64 = 1_u64 << 16;
const MICROS_PER_SEC: u64 = 1_000_000;

/// Default RTP clock rate used for jitter/time conversions when no explicit
/// rate has been configured for the session.
pub fn default_clock_rate_hz(payload_mode: RtpPayloadMode) -> u64 {
    match payload_mode {
        RtpPayloadMode::RawAudio => 8_000,
        // PS/TS/ES/Ehome/XHB/JTT1078 and raw video all use the 90 kHz RTP clock
        // for timestamping; audio sample rate is configured separately when known.
        RtpPayloadMode::Ps
        | RtpPayloadMode::Ts
        | RtpPayloadMode::Es
        | RtpPayloadMode::Ehome
        | RtpPayloadMode::Xhb
        | RtpPayloadMode::Jtt1078
        | RtpPayloadMode::RawVideo
        | RtpPayloadMode::Unknown => 90_000,
    }
}

/// Convert wall-clock milliseconds since the Unix epoch to a 64-bit NTP timestamp.
///
/// `now_ms` must be Unix-epoch milliseconds; callers (drivers) are responsible for
/// providing a wall-clock value, because core is Sans-I/O and cannot read the clock.
pub fn ms_to_ntp_timestamp(now_ms: u64) -> u64 {
    let seconds = now_ms / MS_PER_SEC + NTP_UNIX_EPOCH_DIFF;
    let fraction = (now_ms % MS_PER_SEC) * FRACTION_PER_MS;
    (seconds << 32) | fraction
}

/// Convert a 64-bit NTP timestamp to Unix-epoch microseconds.
fn ntp_to_unix_micros(ntp: u64) -> i64 {
    let seconds = (ntp >> 32).saturating_sub(NTP_UNIX_EPOCH_DIFF);
    let fraction = ntp & 0xffff_ffff;
    let micros = (seconds * MICROS_PER_SEC).saturating_add((fraction * MICROS_PER_SEC) >> 32);
    micros as i64
}

/// Convert an unwrapped RTP timestamp count to media microseconds at `clock_rate_hz`.
fn rtp_to_media_micros(unwrapped_rtp: u64, clock_rate_hz: u64) -> i64 {
    let clock_rate_hz = clock_rate_hz.max(1);
    ((u128::from(unwrapped_rtp) * u128::from(MICROS_PER_SEC)) / u128::from(clock_rate_hz)) as i64
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
    /// RTP timestamp carried in the last received SR.
    last_sr_rtp: u32,
    /// Unwrapped 64-bit RTP timestamp corresponding to `last_sr_rtp`.
    last_sr_unwrapped: u64,
    /// Offset (in microseconds) that aligns the RTP media timeline with the NTP wall clock.
    media_offset_us: Option<i64>,
    /// `now_ms` when the last SR was received.
    last_sr_received_ms: u64,
    /// Interarrival jitter in RTP timestamp units.
    jitter: u64,
    last_transit: Option<u64>,
    /// RTP clock rate used to convert arrival time to RTP timestamp units.
    clock_rate_hz: u64,
    /// Last RTP timestamp that the local sender placed on an outbound packet.
    last_sent_rtp_timestamp: u32,
    /// Unwraps 32-bit RTP timestamps to 64-bit values for SR mapping.
    timestamp_unwrapper: WrapUnwrapper,
    /// Wall-clock offset in milliseconds added to the monotonic `now_ms` when producing
    /// outbound Sender Report NTP timestamps. `RtcpReportState` is Sans-I/O and cannot read
    /// the wall clock, so the driver supplies this offset.
    wall_clock_offset_ms: u64,
}

impl Default for RtcpReportState {
    fn default() -> Self {
        Self::new(90_000)
    }
}

impl RtcpReportState {
    /// Create a new state with the given RTP clock rate (Hz).
    pub fn new(clock_rate_hz: u64) -> Self {
        Self::new_with_offset(clock_rate_hz, 0)
    }

    /// Create a new state with an explicit wall-clock offset for outbound Sender Report
    /// NTP timestamps.
    pub fn new_with_offset(clock_rate_hz: u64, wall_clock_offset_ms: u64) -> Self {
        Self {
            seq: RtpSequenceUnwrapper::new(),
            base_seq: None,
            expected_prior: 0,
            received_prior: 0,
            packets_received: 0,
            last_sr_ntp: 0,
            last_sr_rtp: 0,
            last_sr_unwrapped: 0,
            media_offset_us: None,
            last_sr_received_ms: 0,
            jitter: 0,
            last_transit: None,
            clock_rate_hz,
            last_sent_rtp_timestamp: 0,
            timestamp_unwrapper: WrapUnwrapper::new(32).expect("32-bit wrap is valid"),
            wall_clock_offset_ms,
        }
    }

    /// Register an outbound RTP packet timestamp for the sender report.
    pub fn on_sent(&mut self, rtp_timestamp: u32) {
        self.last_sent_rtp_timestamp = rtp_timestamp;
    }

    /// Update the RTP clock rate used for jitter/time conversions.
    pub fn set_clock_rate_hz(&mut self, clock_rate_hz: u64) {
        if clock_rate_hz != self.clock_rate_hz {
            // Transit times are in RTP-timestamp units at the old rate; drop the
            // baseline so the next packet re-seeds it and no false jitter spike is
            // reported after a mid-stream format switch.
            self.last_transit = None;
        }
        self.clock_rate_hz = clock_rate_hz;
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
        let transit = arrival_rtp_ts.wrapping_sub(u64::from(rtp_timestamp));

        if let Some(prev) = self.last_transit {
            // RFC 3550 A.8: J = J + (|D| - J) / 16.  Compute the difference
            // in the low 32 bits so that timestamp wraparound is handled
            // correctly, then take the signed absolute value.
            let d_u32 = (transit as u32).wrapping_sub(prev as u32);
            let abs_d = (d_u32 as i32).unsigned_abs() as u64;

            let jitter = self.jitter as i64;
            self.jitter = (jitter + (abs_d as i64 - jitter) / 16).max(0) as u64;
        }
        self.last_transit = Some(transit);
    }

    /// Register a received sender report so RR can echo LSR/DLSR and future RTP
    /// timestamps can be mapped to NTP/media timeline.
    pub fn on_sender_report(&mut self, ntp: u64, rtp: u32, now_ms: u64) {
        self.last_sr_ntp = ntp;
        self.last_sr_rtp = rtp;
        self.last_sr_received_ms = now_ms;
        self.last_sr_unwrapped = self.timestamp_unwrapper.unwrap(u64::from(rtp));

        let sr_media_us = rtp_to_media_micros(self.last_sr_unwrapped, self.clock_rate_hz);
        let sr_ntp_us = ntp_to_unix_micros(ntp);
        self.media_offset_us = Some(sr_ntp_us - sr_media_us);
    }

    /// Map a 32-bit RTP timestamp to the NTP wall clock and the media timeline (Unix-epoch
    /// microseconds) using the most recent SR. Returns `None` until an SR has been received.
    ///
    /// The mapping is relative to the last SR and correctly handles 32-bit RTP timestamp wrap
    /// around as well as small backward differences from out-of-order packets such as B-frames.
    pub fn map_rtp_to_ntp_and_media(&self, rtp: u32) -> Option<(u64, i64)> {
        let offset_us = self.media_offset_us?;
        let sr_rtp = self.last_sr_rtp;
        let sr_unwrapped = self.last_sr_unwrapped;

        // Signed 32-bit delta handles wrap-around and out-of-order timestamps near the SR.
        let delta = (rtp as i32).wrapping_sub(sr_rtp as i32) as i64;

        // Compute the media offset from the signed delta directly so negative deltas
        // cannot underflow the absolute u64 media timeline before the SR.
        let delta_media_us = (i128::from(delta) * i128::from(MICROS_PER_SEC))
            / i128::from(self.clock_rate_hz.max(1));
        let media_us = rtp_to_media_micros(sr_unwrapped, self.clock_rate_hz)
            + delta_media_us as i64
            + offset_us;

        let ntp_delta = (i128::from(delta) * (1i128 << 32)) / i128::from(self.clock_rate_hz.max(1));
        let ntp = if ntp_delta >= 0 {
            self.last_sr_ntp.wrapping_add(ntp_delta as u64)
        } else {
            self.last_sr_ntp.wrapping_sub((-ntp_delta) as u64)
        };

        Some((ntp, media_us))
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

    /// Override the wall-clock offset used for outbound Sender Report NTP timestamps.
    pub fn set_wall_clock_offset_ms(&mut self, offset_ms: u64) {
        self.wall_clock_offset_ms = offset_ms;
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
        let wall_clock_ms = now_ms.saturating_add(self.wall_clock_offset_ms);
        RtcpSenderReport {
            ssrc,
            ntp_timestamp: ms_to_ntp_timestamp(wall_clock_ms),
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
        state.on_packet(1, 900, 40); // 30 ms late relative to 10 ms spacing
        let spike_jitter = state.report_block(0, 40).unwrap().jitter;
        assert!(spike_jitter > 0);

        // Smooth stream afterward should bring the jitter estimate back down.
        for i in 2u32..12 {
            let ts = i * 900;
            state.on_packet(i as u16, ts, 10 + u64::from(i) * 10);
        }
        let recovered_jitter = state.report_block(0, 130).unwrap().jitter;
        assert!(recovered_jitter < spike_jitter);
    }

    #[test]
    fn lsr_and_dlsr_echo_received_sender_report() {
        let mut state = RtcpReportState::new(90_000);
        let ntp = ms_to_ntp_timestamp(1000);
        state.on_sender_report(ntp, 0, 1000);

        state.on_packet(0, 0, 1000);
        let block = state.report_block(0x22222222, 1500).unwrap();
        assert_eq!(block.last_sr, ntp_middle_32(ntp));
        assert_eq!(block.delay_since_last_sr, 32_768);
    }

    #[test]
    fn sr_maps_rtp_to_ntp_and_media_timeline() {
        let mut state = RtcpReportState::new(90_000);
        // Wall clock is 10 seconds after the Unix epoch, RTP timestamp is 900_000 (90kHz -> 10s).
        let sr_ntp = ms_to_ntp_timestamp(10_000);
        let sr_rtp: u32 = 900_000;
        state.on_sender_report(sr_ntp, sr_rtp, 10_000);

        // 2 seconds later: RTP timestamp increased by 180_000 ticks.
        let rtp = sr_rtp + 180_000;
        let (ntp, media_us) = state.map_rtp_to_ntp_and_media(rtp).unwrap();
        assert_eq!(media_us, 12_000_000);
        assert_eq!(ntp, ms_to_ntp_timestamp(12_000));
    }

    #[test]
    fn sr_mapping_handles_32bit_wraparound_and_backward_packets() {
        let mut state = RtcpReportState::new(90_000);
        // The SR is sent at 1 second of wall-clock time, one second before the RTP timestamp
        // wraps from u32::MAX to 0.
        let sr_ntp = ms_to_ntp_timestamp(1_000);
        let sr_rtp: u32 = u32::MAX - 90_000 + 1;
        state.on_sender_report(sr_ntp, sr_rtp, 1_000);

        // Wrapped forward by 180_000 ticks -> 2 seconds of media time after the SR.
        let rtp = 90_000u32;
        let (ntp, media_us) = state.map_rtp_to_ntp_and_media(rtp).unwrap();
        assert_eq!(media_us, 3_000_000);
        assert!(ntp > sr_ntp);

        // A B-frame-like packet 50 ms before the SR still maps correctly.
        let backward_rtp = sr_rtp.wrapping_sub(4_500);
        let (_, backward_media_us) = state.map_rtp_to_ntp_and_media(backward_rtp).unwrap();
        assert_eq!(backward_media_us, 1_000_000 - 50_000);
    }

    #[test]
    fn map_rtp_to_ntp_and_media_returns_none_before_sr() {
        let state = RtcpReportState::new(90_000);
        assert!(state.map_rtp_to_ntp_and_media(0).is_none());
    }

    #[test]
    fn sr_mapping_negative_delta_does_not_underflow_at_stream_start() {
        let mut state = RtcpReportState::new(90_000);
        // SR at stream start with RTP timestamp 0. A packet from just before the
        // wrap boundary (45_000 ticks earlier) must map to a small negative media time.
        state.on_sender_report(ms_to_ntp_timestamp(0), 0, 0);

        let backward_rtp = 0u32.wrapping_sub(45_000);
        let (ntp, media_us) = state.map_rtp_to_ntp_and_media(backward_rtp).unwrap();
        assert_eq!(media_us, -500_000);
        assert!(ntp < ms_to_ntp_timestamp(0));
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

    #[test]
    fn jitter_handles_32bit_timestamp_wraparound() {
        let mut state = RtcpReportState::new(90_000);
        // 10 ms spacing right before the 32-bit wrap boundary.
        state.on_packet(0, u32::MAX - 900, 0);
        state.on_packet(1, u32::MAX, 10);
        state.on_packet(2, 900, 20); // wrapped
        let block = state.report_block(0, 30).unwrap();
        assert_eq!(block.jitter, 0);
    }

    #[test]
    fn sender_report_ntp_uses_wall_clock_offset() {
        // The driver supplies the Unix-epoch wall-clock timestamp at runtime start as
        // the offset. `now_ms` is monotonic-from-start, so adding the offset yields
        // the real wall-clock time used for the RTCP Sender Report NTP field.
        let offset_ms = 1_720_000_000_000u64;
        let mut state = RtcpReportState::new_with_offset(90_000, offset_ms);
        state.on_sent(0);

        let sr = state.sender_report(0, 0, 0, 0, None);
        assert_eq!(sr.ntp_timestamp, ms_to_ntp_timestamp(offset_ms));

        let sr = state.sender_report(0, 0, 0, 1_000, None);
        assert_eq!(sr.ntp_timestamp, ms_to_ntp_timestamp(offset_ms + 1_000));
    }
}
