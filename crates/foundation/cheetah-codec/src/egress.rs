use crate::prelude::*;
use crate::{AVFrame, CodecId, MediaKind, Timebase};
use alloc::collections::VecDeque;

fn round_half_away_from_zero(value: i128, divisor: i128) -> i128 {
    let half = divisor / 2;
    if value >= 0 {
        (value + half) / divisor
    } else {
        (value - half) / divisor
    }
}

fn timebase_value_to_millis(tb: Timebase, value: i64) -> i64 {
    if value == 0 {
        return 0;
    }
    let den = i128::from(tb.den.max(1));
    let num = i128::from(tb.num.max(1));
    let scaled = i128::from(value) * num * 1_000_i128;
    round_half_away_from_zero(scaled, den) as i64
}

pub fn millis_to_rtmp_timestamp_ms(millis: i64) -> u32 {
    if millis <= 0 {
        return 0;
    }
    millis.min(i64::from(u32::MAX)) as u32
}

pub fn dts_to_rtmp_timestamp_ms(dts: i64, timebase: Timebase) -> u32 {
    let dts_ms = timebase_value_to_millis(timebase, dts);
    millis_to_rtmp_timestamp_ms(dts_ms)
}

pub fn frame_dts_to_rtmp_timestamp_ms(frame: &AVFrame) -> u32 {
    dts_to_rtmp_timestamp_ms(frame.dts, frame.timebase)
}

pub fn frame_composition_time_ms(frame: &AVFrame) -> i32 {
    let pts_ms = timebase_value_to_millis(frame.timebase, frame.pts);
    let dts_ms = timebase_value_to_millis(frame.timebase, frame.dts);
    // RTMP video CTS is a signed 24-bit offset.
    pts_ms.saturating_sub(dts_ms).clamp(-0x80_0000, 0x7F_FFFF) as i32
}

/// Selects the primary and secondary timestamps for cross-protocol egress.
///
/// RTP/RTMP/WebRTC all require a monotonic decode timeline on the wire. The
/// primary value is therefore the DTS for both audio and video; the PTS is
/// retained as the secondary fallback and used for RTMP composition offsets.
pub fn select_egress_timestamps(_media_kind: MediaKind, pts: i64, dts: i64) -> (i64, i64) {
    (dts, pts)
}

pub fn media_ts_to_rtp_ticks(
    primary: i64,
    secondary: i64,
    timebase: Timebase,
    clock_rate: u32,
) -> u32 {
    if clock_rate == 0 {
        return 0;
    }
    let ts = if primary >= 0 {
        primary
    } else if secondary >= 0 {
        secondary
    } else {
        return 0;
    };
    let den = i128::from(timebase.den.max(1));
    let num = i128::from(timebase.num.max(1));
    let value = i128::from(ts)
        .saturating_mul(num)
        .saturating_mul(i128::from(clock_rate))
        / den;
    if value <= 0 {
        0
    } else {
        value as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimestampRepairResult {
    pub timestamp: u32,
    pub repaired: bool,
}

pub fn repair_monotonic_timestamp(
    raw_timestamp: u32,
    last_timestamp: Option<u32>,
    backward_repair_threshold: u32,
) -> TimestampRepairResult {
    let Some(last) = last_timestamp else {
        return TimestampRepairResult {
            timestamp: raw_timestamp,
            repaired: false,
        };
    };

    if raw_timestamp == last {
        return TimestampRepairResult {
            timestamp: last.wrapping_add(1),
            repaired: true,
        };
    }
    if raw_timestamp < last {
        let backward = last.wrapping_sub(raw_timestamp);
        if backward <= backward_repair_threshold {
            return TimestampRepairResult {
                timestamp: last.wrapping_add(1),
                repaired: true,
            };
        }
    }

    TimestampRepairResult {
        timestamp: raw_timestamp,
        repaired: false,
    }
}

pub fn should_sample_timestamp_repair(repair_count: u64) -> bool {
    repair_count <= 3 || repair_count.is_power_of_two() || repair_count.is_multiple_of(1024)
}

pub fn should_emit_alert_threshold(count: u64, threshold: u64) -> bool {
    threshold > 0 && count >= threshold && (count == threshold || count.is_multiple_of(threshold))
}

/// Aligns audio/video timestamps for cross-protocol egress by tracking the
/// epoch offset between the first video and audio frames. Ensures both tracks
/// share a common time origin so players don't experience A/V desync.
pub struct AvSyncAligner {
    video_epoch_us: Option<i64>,
    audio_epoch_us: Option<i64>,
    sync_offset_us: i64,
    synced: bool,
}

impl AvSyncAligner {
    pub fn new() -> Self {
        Self {
            video_epoch_us: None,
            audio_epoch_us: None,
            sync_offset_us: 0,
            synced: false,
        }
    }

    /// Record the first frame's DTS for each media kind. Once both are known,
    /// compute the sync offset.
    pub fn on_frame(&mut self, media_kind: MediaKind, dts_us: i64) {
        if self.synced {
            return;
        }
        match media_kind {
            MediaKind::Video if self.video_epoch_us.is_none() => {
                self.video_epoch_us = Some(dts_us);
            }
            MediaKind::Audio if self.audio_epoch_us.is_none() => {
                self.audio_epoch_us = Some(dts_us);
            }
            _ => {}
        }
        if let (Some(v), Some(a)) = (self.video_epoch_us, self.audio_epoch_us) {
            self.sync_offset_us = a.saturating_sub(v);
            self.synced = true;
        }
    }

    /// Adjust a DTS value for egress so audio and video share a common epoch.
    /// Video timestamps are unchanged; audio timestamps are shifted to align.
    pub fn adjust(&self, media_kind: MediaKind, dts_us: i64) -> i64 {
        if !self.synced {
            return dts_us;
        }
        match media_kind {
            MediaKind::Audio => dts_us.saturating_sub(self.sync_offset_us),
            _ => dts_us,
        }
    }

    pub fn is_synced(&self) -> bool {
        self.synced
    }

    pub fn offset_us(&self) -> i64 {
        self.sync_offset_us
    }
}

impl Default for AvSyncAligner {
    fn default() -> Self {
        Self::new()
    }
}

/// Generates DTS from PTS-only streams using a sorting window approach.
/// Buffers N frames of PTS values, then outputs the minimum as DTS.
/// This handles B-frame reordering (IBBP patterns) more accurately than
/// step estimation alone.
pub struct SortingWindowDtsGenerator {
    window: VecDeque<i64>,
    window_size: usize,
    last_output_dts: i64,
    initialized: bool,
}

impl SortingWindowDtsGenerator {
    pub fn new(window_size: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size.max(1)),
            window_size: window_size.max(1),
            last_output_dts: 0,
            initialized: false,
        }
    }

    /// Feed a PTS value and get back a DTS. Returns `None` while the window
    /// is still filling up (buffering phase).
    pub fn push(&mut self, pts: i64) -> Option<i64> {
        self.window.push_back(pts);
        if self.window.len() < self.window_size {
            return None;
        }
        let min_pts = *self.window.iter().min().unwrap();
        let dts = if self.initialized {
            min_pts.max(self.last_output_dts + 1)
        } else {
            self.initialized = true;
            min_pts
        };
        self.window.pop_front();
        self.last_output_dts = dts;
        Some(dts)
    }

    /// Flush remaining buffered frames (call at end-of-stream or discontinuity).
    pub fn flush(&mut self) -> Vec<i64> {
        let mut out = Vec::with_capacity(self.window.len());
        while !self.window.is_empty() {
            let min_pts = *self.window.iter().min().unwrap();
            let dts = if self.initialized {
                min_pts.max(self.last_output_dts + 1)
            } else {
                self.initialized = true;
                min_pts
            };
            // Remove the element equal to min_pts
            if let Some(pos) = self.window.iter().position(|&v| v == min_pts) {
                self.window.remove(pos);
            } else {
                self.window.pop_front();
            }
            self.last_output_dts = dts;
            out.push(dts);
        }
        out
    }

    pub fn reset(&mut self) {
        self.window.clear();
        self.last_output_dts = 0;
        self.initialized = false;
    }
}

/// Result of incremental RTP timestamp generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpEgressTimestamp {
    /// RTP timestamp based on DTS progression (for decode ordering).
    pub rtp_timestamp: u32,
    /// PTS-based RTP timestamp (for display ordering, used as actual RTP ts for video).
    pub rtp_timestamp_pts: u32,
}

/// Generates RTP timestamps incrementally to avoid cumulative rounding errors
/// when converting from media timebases (e.g., 1/1000 ms) to RTP clock rates
/// (e.g., 90000 Hz). Tracks fractional ticks to prevent drift over long sessions.
pub struct IncrementalRtpTimestampGenerator {
    last_dts_us: i64,
    rtp_timestamp: u32,
    clock_rate: u32,
    fractional_ticks: f64,
    initialized: bool,
}

impl IncrementalRtpTimestampGenerator {
    pub fn new(clock_rate: u32) -> Self {
        Self {
            last_dts_us: 0,
            rtp_timestamp: 0,
            clock_rate: clock_rate.max(1),
            fractional_ticks: 0.0,
            initialized: false,
        }
    }

    /// Reset the generator (e.g., on discontinuity).
    pub fn reset(&mut self, dts_us: i64) {
        self.last_dts_us = dts_us;
        self.fractional_ticks = 0.0;
    }

    /// Generate next RTP timestamp from media microsecond timestamps.
    /// Returns both DTS-based and PTS-based RTP timestamps.
    pub fn next(&mut self, dts_us: i64, pts_us: i64) -> RtpEgressTimestamp {
        if !self.initialized {
            self.initialized = true;
            self.last_dts_us = dts_us;
            return RtpEgressTimestamp {
                rtp_timestamp: self.rtp_timestamp,
                rtp_timestamp_pts: self.rtp_timestamp,
            };
        }

        let delta_us = dts_us.saturating_sub(self.last_dts_us);
        let exact_ticks =
            delta_us as f64 * self.clock_rate as f64 / 1_000_000.0 + self.fractional_ticks;
        let int_ticks = round_f64(exact_ticks) as i64;
        self.fractional_ticks = exact_ticks - int_ticks as f64;

        self.rtp_timestamp = self.rtp_timestamp.wrapping_add(int_ticks as u32);
        self.last_dts_us = dts_us;

        // PTS offset for B-frames: compute PTS-based timestamp
        let pts_delta_us = pts_us.saturating_sub(dts_us);
        let pts_offset =
            round_f64(pts_delta_us as f64 * self.clock_rate as f64 / 1_000_000.0) as i32;
        let rtp_timestamp_pts = self.rtp_timestamp.wrapping_add(pts_offset as u32);

        RtpEgressTimestamp {
            rtp_timestamp: self.rtp_timestamp,
            rtp_timestamp_pts,
        }
    }
}

/// Estimates video frame rate from PTS deltas by collecting samples and averaging.
/// Used when metadata-declared frame rate is missing or inaccurate.
pub struct FrameRateEstimator {
    samples: VecDeque<i64>,
    max_samples: usize,
    last_pts_us: Option<i64>,
    /// Number of initial frames to skip before collecting samples (ABL: 15).
    warmup_frames: usize,
    /// Frames seen so far (for warmup tracking).
    frames_seen: usize,
    /// Minimum FPS clamp (default 1.0).
    min_fps: f64,
    /// Maximum FPS clamp (ABL: 120).
    max_fps: f64,
}

impl FrameRateEstimator {
    pub fn new(max_samples: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(max_samples.max(1)),
            max_samples: max_samples.max(1),
            last_pts_us: None,
            warmup_frames: 0,
            frames_seen: 0,
            min_fps: 1.0,
            max_fps: 120.0,
        }
    }

    /// Create with ABL-style defaults: 15 warmup frames, 120 max fps.
    pub fn with_abl_defaults(max_samples: usize) -> Self {
        Self {
            warmup_frames: 15,
            max_fps: 120.0,
            min_fps: 1.0,
            ..Self::new(max_samples)
        }
    }

    /// Set warmup frames count.
    pub fn set_warmup_frames(&mut self, n: usize) {
        self.warmup_frames = n;
    }

    /// Set min/max FPS clamp.
    pub fn set_fps_clamp(&mut self, min: f64, max: f64) {
        self.min_fps = min;
        self.max_fps = max;
    }

    /// Feed a video frame's PTS (microseconds). Returns estimated FPS once
    /// enough samples are collected, or None while still warming up.
    pub fn on_frame(&mut self, pts_us: i64) -> Option<f64> {
        self.frames_seen += 1;

        if let Some(last) = self.last_pts_us {
            let delta = pts_us.saturating_sub(last);
            // Only collect samples after warmup period
            if delta > 0 && delta < 1_000_000 && self.frames_seen > self.warmup_frames {
                if self.samples.len() >= self.max_samples {
                    self.samples.pop_front();
                }
                self.samples.push_back(delta);
            }
        }
        self.last_pts_us = Some(pts_us);
        self.estimated_fps()
    }

    /// Current estimated FPS (clamped), or None if insufficient samples.
    pub fn estimated_fps(&self) -> Option<f64> {
        if self.samples.len() < self.max_samples / 2 {
            return None;
        }
        let sum: i64 = self.samples.iter().sum();
        if sum <= 0 {
            return None;
        }
        let avg_us = sum as f64 / self.samples.len() as f64;
        let fps = 1_000_000.0 / avg_us;
        Some(clamp_f64(fps, self.min_fps, self.max_fps))
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.last_pts_us = None;
        self.frames_seen = 0;
    }
}

// ─── RTP timestamp strategy: live vs replay ─────────────────────────────────

/// Scenario mode for RTP timestamp derivation.
///
/// Live streams use monotonically increasing timestamps derived from the
/// canonical media timeline (source clock normalization). Replay streams
/// prefer source frame number or source PTS to derive RTP timestamps so
/// that the receiver can reconstruct the original presentation timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpTimestampMode {
    /// Live scenario: derive RTP timestamp from canonical DTS/PTS using
    /// the codec clock rate. This is the existing monotonic behaviour.
    Live,
    /// Replay scenario: prefer source frame number or source PTS to
    /// derive the RTP timestamp. If a frame number is provided, the RTP
    /// timestamp is `frame_number * samples_per_frame`. Otherwise, the
    /// source PTS (in the source timebase) is converted to the codec
    /// clock rate.
    Replay,
}

/// Input for the pure RTP timestamp computation function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpTimestampInput {
    /// Canonical PTS in the frame's timebase ticks.
    pub pts: i64,
    /// Canonical DTS in the frame's timebase ticks.
    pub dts: i64,
    /// The frame's timebase (e.g., 1/1000 for ms, 1/90000 for RTP video).
    pub timebase: Timebase,
    /// Media kind (audio/video).
    pub media_kind: MediaKind,
    /// Codec identifier — used to determine the RTP clock rate.
    pub codec: CodecId,
    /// The RTP clock rate for this codec (e.g., 90000 for video, 48000
    /// for Opus, 8000 for G711).
    pub clock_rate: u32,
    /// Timestamp mode: live or replay.
    pub mode: RtpTimestampMode,
    /// For replay mode: optional source frame number. When present, the
    /// RTP timestamp is derived as `frame_number * samples_per_frame`.
    pub source_frame_number: Option<u64>,
    /// For replay mode: optional source PTS in the original source
    /// timebase. When `source_frame_number` is not available, this is
    /// converted to the codec clock rate.
    pub source_pts: Option<i64>,
    /// For replay mode: the source timebase (needed to convert
    /// `source_pts` to the codec clock rate). Defaults to the frame's
    /// timebase if not specified.
    pub source_timebase: Option<Timebase>,
    /// Samples per frame for audio codecs. Used when deriving timestamp
    /// from frame number. For G711 20ms: 160, for Opus 20ms: 960.
    pub samples_per_frame: Option<u32>,
}

/// Compute the RTP timestamp for a media frame.
///
/// This is a pure function that centralizes the timestamp derivation
/// logic for WebRTC egress. The module should call this instead of
/// performing ad-hoc timestamp arithmetic.
///
/// # Strategy
///
/// - **Live mode**: Uses the canonical PTS/DTS converted to the codec
///   clock rate. For both video and audio, DTS is preferred to keep the
///   RTP timestamp stream monotonic (matching `select_egress_timestamps`
///   semantics).
///
/// - **Replay mode**: Prefers `source_frame_number * samples_per_frame`
///   when both are available. Falls back to converting `source_pts` from
///   the source timebase to the codec clock rate. If neither source hint
///   is available, falls back to live-mode behaviour.
pub fn compute_rtp_timestamp(input: &RtpTimestampInput) -> u32 {
    if input.clock_rate == 0 {
        return 0;
    }

    match input.mode {
        RtpTimestampMode::Live => compute_rtp_timestamp_live(input),
        RtpTimestampMode::Replay => compute_rtp_timestamp_replay(input),
    }
}

fn compute_rtp_timestamp_live(input: &RtpTimestampInput) -> u32 {
    let (primary, secondary) = select_egress_timestamps(input.media_kind, input.pts, input.dts);
    media_ts_to_rtp_ticks(primary, secondary, input.timebase, input.clock_rate)
}

fn compute_rtp_timestamp_replay(input: &RtpTimestampInput) -> u32 {
    // Priority 1: source frame number * samples_per_frame
    if let (Some(frame_number), Some(spf)) = (input.source_frame_number, input.samples_per_frame) {
        let ticks = (frame_number as u128).saturating_mul(spf as u128);
        return (ticks % (u32::MAX as u128 + 1)) as u32;
    }

    // Priority 2: source PTS converted to codec clock rate
    if let Some(source_pts) = input.source_pts {
        let src_tb = input.source_timebase.unwrap_or(input.timebase);
        let rtp_tb = Timebase::new(1, input.clock_rate);
        let ticks = crate::time::TimebaseConverter::convert(source_pts, src_tb, rtp_tb);
        return if ticks < 0 { 0 } else { ticks as u32 };
    }

    // Fallback: use live-mode derivation
    compute_rtp_timestamp_live(input)
}

/// Calculate the RTP timestamp step (increment per packet) for an audio
/// codec given its sample rate and packet duration (ptime).
///
/// This is a pure function that should be used instead of hardcoding
/// timestamp steps in the module.
///
/// # Examples
///
/// ```
/// use cheetah_codec::egress::audio_rtp_timestamp_step;
///
/// // G711 at 8000 Hz, 20ms ptime → 160 samples
/// assert_eq!(audio_rtp_timestamp_step(8000, 20), 160);
///
/// // G711 at 8000 Hz, 40ms ptime → 320 samples
/// assert_eq!(audio_rtp_timestamp_step(8000, 40), 320);
///
/// // Opus at 48000 Hz, 20ms ptime → 960 samples
/// assert_eq!(audio_rtp_timestamp_step(48000, 20), 960);
/// ```
pub fn audio_rtp_timestamp_step(sample_rate: u32, ptime_ms: u32) -> u32 {
    // step = sample_rate * ptime_ms / 1000
    (sample_rate as u64 * ptime_ms as u64 / 1000) as u32
}

/// Returns the standard RTP clock rate for a given codec.
///
/// - Video codecs: 90000 Hz
/// - Opus: 48000 Hz
/// - G711A/G711U: 8000 Hz
/// - Other audio: 0 (unknown)
pub fn codec_rtp_clock_rate(codec: CodecId) -> u32 {
    match codec {
        CodecId::H264
        | CodecId::H265
        | CodecId::H266
        | CodecId::VP8
        | CodecId::VP9
        | CodecId::AV1 => 90_000,
        CodecId::Opus => 48_000,
        CodecId::G711A | CodecId::G711U => 8_000,
        _ => 0,
    }
}

/// Returns the default samples-per-frame for a codec at its standard
/// ptime (20ms).
///
/// - Opus at 48kHz, 20ms → 960
/// - G711 at 8kHz, 20ms → 160
pub fn codec_default_samples_per_frame(codec: CodecId) -> Option<u32> {
    match codec {
        CodecId::Opus => Some(audio_rtp_timestamp_step(48_000, 20)), // 960
        CodecId::G711A | CodecId::G711U => Some(audio_rtp_timestamp_step(8_000, 20)), // 160
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::{CodecId, FrameFormat, TrackId};

    use super::*;

    #[test]
    fn rtmp_timestamp_rounds_from_non_ms_timebase() {
        assert_eq!(
            dts_to_rtmp_timestamp_ms(9_000, Timebase::new(1, 90_000)),
            100
        );
        assert_eq!(dts_to_rtmp_timestamp_ms(45, Timebase::new(1, 30)), 1_500);
    }

    #[test]
    fn rtmp_timestamp_and_cts_preserve_negative_offset() {
        let frame = AVFrame::new(
            TrackId(7),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            100,
            160,
            Timebase::new(1, 1_000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        );
        assert_eq!(frame_dts_to_rtmp_timestamp_ms(&frame), 160);
        assert_eq!(frame_composition_time_ms(&frame), -60);
    }

    #[test]
    fn rtmp_cts_clamps_to_signed_24bit_range() {
        let frame = AVFrame::new(
            TrackId(8),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            9_000_000,
            0,
            Timebase::new(1, 1_000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        );
        assert_eq!(frame_composition_time_ms(&frame), 0x7F_FFFF);

        let frame = AVFrame::new(
            TrackId(9),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            9_000_000,
            Timebase::new(1, 1_000),
            Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        );
        assert_eq!(frame_composition_time_ms(&frame), -0x80_0000);
    }

    #[test]
    fn media_ts_to_rtp_ticks_prefers_dts_for_video_and_audio() {
        let (video_primary, video_secondary) =
            select_egress_timestamps(MediaKind::Video, 9_000, 3_000);
        assert_eq!(video_primary, 3_000);
        assert_eq!(video_secondary, 9_000);
        let video_ticks = media_ts_to_rtp_ticks(
            video_primary,
            video_secondary,
            Timebase::new(1, 90_000),
            90_000,
        );
        assert_eq!(video_ticks, 3_000);

        let (audio_primary, audio_secondary) =
            select_egress_timestamps(MediaKind::Audio, 9_000, 3_000);
        assert_eq!(audio_primary, 3_000);
        assert_eq!(audio_secondary, 9_000);
        let audio_ticks = media_ts_to_rtp_ticks(
            audio_primary,
            audio_secondary,
            Timebase::new(1, 90_000),
            90_000,
        );
        assert_eq!(audio_ticks, 3_000);
    }

    #[test]
    fn media_ts_to_rtp_ticks_respects_zero_and_wrap() {
        let zero = media_ts_to_rtp_ticks(0, 1_024, Timebase::new(1, 1_000), 48_000);
        assert_eq!(zero, 0);

        let wrapped =
            media_ts_to_rtp_ticks(i64::from(u32::MAX) + 1, 0, Timebase::new(1, 90_000), 90_000);
        assert_eq!(wrapped, 0);
    }

    #[test]
    fn repair_monotonic_timestamp_repairs_duplicate_and_small_backward_jitter() {
        let duplicate = repair_monotonic_timestamp(1_000, Some(1_000), 3_000);
        assert_eq!(duplicate.timestamp, 1_001);
        assert!(duplicate.repaired);

        let backward = repair_monotonic_timestamp(950, Some(1_000), 3_000);
        assert_eq!(backward.timestamp, 1_001);
        assert!(backward.repaired);
    }

    #[test]
    fn repair_monotonic_timestamp_keeps_large_resets_and_wraparound() {
        let reset = repair_monotonic_timestamp(0, Some(100_000), 3_000);
        assert_eq!(reset.timestamp, 0);
        assert!(!reset.repaired);

        let wrapped = repair_monotonic_timestamp(12, Some(u32::MAX - 5), 3_000);
        assert_eq!(wrapped.timestamp, 12);
        assert!(!wrapped.repaired);
    }

    #[test]
    fn threshold_and_sampling_helpers_match_policy() {
        assert!(should_sample_timestamp_repair(1));
        assert!(should_sample_timestamp_repair(2));
        assert!(should_sample_timestamp_repair(3));
        assert!(should_sample_timestamp_repair(4));
        assert!(!should_sample_timestamp_repair(5));
        assert!(should_sample_timestamp_repair(8));
        assert!(should_sample_timestamp_repair(1024));

        assert!(!should_emit_alert_threshold(63, 64));
        assert!(should_emit_alert_threshold(64, 64));
        assert!(should_emit_alert_threshold(128, 64));
    }

    #[test]
    fn sorting_window_dts_generator_handles_ibbp_pattern() {
        // Simulate decode-order PTS for IBBP: I=0, P=3000, B=1000, B=2000, P=6000
        // Window size 3: buffers 3 PTS values, outputs min as DTS
        let mut gen = SortingWindowDtsGenerator::new(3);

        // Buffering phase
        assert_eq!(gen.push(0), None);
        assert_eq!(gen.push(3000), None);
        // Window full: [0, 3000, 1000] → min=0, pop_front(0)
        let dts = gen.push(1000).unwrap();
        assert_eq!(dts, 0);
        // Window: [3000, 1000, 2000] → min=1000, pop_front(3000)
        let dts = gen.push(2000).unwrap();
        assert_eq!(dts, 1000);
        // Window: [1000, 2000, 6000] → min=1000, but last_dts=1000 so dts=1001
        let dts = gen.push(6000).unwrap();
        assert_eq!(dts, 1001);

        // All output DTS are monotonically increasing
        let remaining = gen.flush();
        let mut prev = 1001i64;
        for dts in &remaining {
            assert!(*dts > prev, "DTS must be monotonic: {dts} > {prev}");
            prev = *dts;
        }
    }

    #[test]
    fn av_sync_aligner_compensates_audio_video_epoch_offset() {
        let mut aligner = AvSyncAligner::new();
        // Video starts at 100ms, audio starts at 150ms (50ms late)
        aligner.on_frame(MediaKind::Video, 100_000);
        assert!(!aligner.is_synced());
        aligner.on_frame(MediaKind::Audio, 150_000);
        assert!(aligner.is_synced());
        assert_eq!(aligner.offset_us(), 50_000);

        // Video unchanged
        assert_eq!(aligner.adjust(MediaKind::Video, 200_000), 200_000);
        // Audio shifted back by 50ms to align with video epoch
        assert_eq!(aligner.adjust(MediaKind::Audio, 200_000), 150_000);
    }

    #[test]
    fn incremental_rtp_generator_no_cumulative_drift_over_one_hour() {
        // Simulate 1 hour of 30fps video from RTMP (1/1000 timebase) → RTSP (90kHz)
        let mut gen = IncrementalRtpTimestampGenerator::new(90_000);
        let frame_duration_us: i64 = 33_333; // ~30fps

        // Send 108000 frames (1 hour at 30fps)
        let num_frames: i64 = 108_000;
        for i in 0..num_frames {
            let dts_us = i * frame_duration_us;
            gen.next(dts_us, dts_us);
        }

        // Expected: total_duration_us * clock_rate / 1_000_000
        let total_us = (num_frames - 1) * frame_duration_us;
        let expected_ticks = round_f64(total_us as f64 * 90_000.0 / 1_000_000.0) as u32;
        let actual_ticks = gen.rtp_timestamp;

        // Allow at most 1 tick of error over 1 hour
        let diff = (actual_ticks as i64 - expected_ticks as i64).unsigned_abs();
        assert!(
            diff <= 1,
            "1-hour drift: expected {expected_ticks}, got {actual_ticks}, diff={diff}"
        );
    }

    #[test]
    fn incremental_rtp_generator_b_frame_pts_offset() {
        let mut gen = IncrementalRtpTimestampGenerator::new(90_000);
        // Frame 0: DTS=0, PTS=0
        let ts0 = gen.next(0, 0);
        assert_eq!(ts0.rtp_timestamp, 0);
        assert_eq!(ts0.rtp_timestamp_pts, 0);

        // Frame 1: DTS=33333us, PTS=66666us (B-frame reorder: PTS > DTS)
        let ts1 = gen.next(33_333, 66_666);
        assert_eq!(ts1.rtp_timestamp, 3000); // 33333 * 90000 / 1000000 = 3000
                                             // PTS offset = (66666-33333) * 90000 / 1000000 = 3000
        assert_eq!(ts1.rtp_timestamp_pts, 6000);
    }

    #[test]
    fn frame_rate_estimator_detects_30fps() {
        let mut est = FrameRateEstimator::new(10);
        // Feed 30fps frames (33333us apart)
        for i in 0..10 {
            est.on_frame(i * 33_333);
        }
        let fps = est.estimated_fps().unwrap();
        assert!((fps - 30.0).abs() < 1.0, "expected ~30fps, got {fps}");
    }

    // ─── RTP timestamp strategy tests ───────────────────────────────────────

    #[test]
    fn audio_rtp_timestamp_step_g711_20ms() {
        // G711 at 8000 Hz, 20ms ptime → 160 samples
        assert_eq!(audio_rtp_timestamp_step(8_000, 20), 160);
    }

    #[test]
    fn audio_rtp_timestamp_step_g711_40ms() {
        // G711 at 8000 Hz, 40ms ptime → 320 samples
        assert_eq!(audio_rtp_timestamp_step(8_000, 40), 320);
    }

    #[test]
    fn audio_rtp_timestamp_step_opus_20ms() {
        // Opus at 48000 Hz, 20ms ptime → 960 samples
        assert_eq!(audio_rtp_timestamp_step(48_000, 20), 960);
    }

    #[test]
    fn codec_rtp_clock_rate_returns_correct_values() {
        assert_eq!(codec_rtp_clock_rate(CodecId::H264), 90_000);
        assert_eq!(codec_rtp_clock_rate(CodecId::H265), 90_000);
        assert_eq!(codec_rtp_clock_rate(CodecId::VP8), 90_000);
        assert_eq!(codec_rtp_clock_rate(CodecId::Opus), 48_000);
        assert_eq!(codec_rtp_clock_rate(CodecId::G711A), 8_000);
        assert_eq!(codec_rtp_clock_rate(CodecId::G711U), 8_000);
    }

    #[test]
    fn codec_default_samples_per_frame_values() {
        assert_eq!(codec_default_samples_per_frame(CodecId::Opus), Some(960));
        assert_eq!(codec_default_samples_per_frame(CodecId::G711A), Some(160));
        assert_eq!(codec_default_samples_per_frame(CodecId::G711U), Some(160));
        assert_eq!(codec_default_samples_per_frame(CodecId::H264), None);
    }

    #[test]
    fn compute_rtp_timestamp_live_video_uses_dts() {
        let input = RtpTimestampInput {
            pts: 9_000,
            dts: 6_000,
            timebase: Timebase::new(1, 90_000),
            media_kind: MediaKind::Video,
            codec: CodecId::H264,
            clock_rate: 90_000,
            mode: RtpTimestampMode::Live,
            source_frame_number: None,
            source_pts: None,
            source_timebase: None,
            samples_per_frame: None,
        };
        // Video uses DTS as primary → 6000 ticks at 90kHz/90kHz = 6000
        assert_eq!(compute_rtp_timestamp(&input), 6_000);
    }

    #[test]
    fn compute_rtp_timestamp_live_audio_uses_dts() {
        let input = RtpTimestampInput {
            pts: 9_000,
            dts: 6_000,
            timebase: Timebase::new(1, 90_000),
            media_kind: MediaKind::Audio,
            codec: CodecId::Opus,
            clock_rate: 48_000,
            mode: RtpTimestampMode::Live,
            source_frame_number: None,
            source_pts: None,
            source_timebase: None,
            samples_per_frame: None,
        };
        // Audio uses DTS as primary → 6000 ticks at 90kHz → 48kHz
        // 6000 * 1/90000 = 66.67ms → 66.67ms * 48000 = 3200
        let result = compute_rtp_timestamp(&input);
        assert_eq!(result, 3_200);
    }

    #[test]
    fn replay_timestamp_is_derived_from_source_frame_time() {
        // Replay with frame number: frame 5, Opus 960 samples/frame
        let input = RtpTimestampInput {
            pts: 100_000, // canonical PTS (should be ignored)
            dts: 100_000,
            timebase: Timebase::new(1, 1_000),
            media_kind: MediaKind::Audio,
            codec: CodecId::Opus,
            clock_rate: 48_000,
            mode: RtpTimestampMode::Replay,
            source_frame_number: Some(5),
            source_pts: None,
            source_timebase: None,
            samples_per_frame: Some(960),
        };
        // frame_number * samples_per_frame = 5 * 960 = 4800
        assert_eq!(compute_rtp_timestamp(&input), 4_800);
    }

    #[test]
    fn replay_timestamp_from_source_pts_when_no_frame_number() {
        // Replay with source PTS but no frame number
        let input = RtpTimestampInput {
            pts: 999, // canonical (should be ignored in replay)
            dts: 999,
            timebase: Timebase::new(1, 1_000),
            media_kind: MediaKind::Audio,
            codec: CodecId::G711A,
            clock_rate: 8_000,
            mode: RtpTimestampMode::Replay,
            source_frame_number: None,
            source_pts: Some(100), // 100ms in source timebase
            source_timebase: Some(Timebase::new(1, 1_000)),
            samples_per_frame: Some(160),
        };
        // source_pts=100 at 1/1000 → convert to 8000Hz: 100 * 8000/1000 = 800
        assert_eq!(compute_rtp_timestamp(&input), 800);
    }

    #[test]
    fn replay_timestamp_falls_back_to_live_when_no_source_hints() {
        // Replay mode but no source_frame_number and no source_pts
        let input = RtpTimestampInput {
            pts: 9_000,
            dts: 9_000,
            timebase: Timebase::new(1, 90_000),
            media_kind: MediaKind::Video,
            codec: CodecId::H264,
            clock_rate: 90_000,
            mode: RtpTimestampMode::Replay,
            source_frame_number: None,
            source_pts: None,
            source_timebase: None,
            samples_per_frame: None,
        };
        // Falls back to live mode: PTS=9000 at 90kHz → 9000
        assert_eq!(compute_rtp_timestamp(&input), 9_000);
    }

    #[test]
    fn replay_g711_frame_number_derives_correct_timestamp() {
        // G711 replay: frame 10, 20ms ptime, 8kHz → step=160
        let input = RtpTimestampInput {
            pts: 200_000, // 200ms canonical (ignored)
            dts: 200_000,
            timebase: Timebase::new(1, 1_000),
            media_kind: MediaKind::Audio,
            codec: CodecId::G711A,
            clock_rate: 8_000,
            mode: RtpTimestampMode::Replay,
            source_frame_number: Some(10),
            source_pts: None,
            source_timebase: None,
            samples_per_frame: Some(160), // 8000 * 20 / 1000
        };
        // 10 * 160 = 1600
        assert_eq!(compute_rtp_timestamp(&input), 1_600);
    }

    #[test]
    fn compute_rtp_timestamp_zero_clock_rate_returns_zero() {
        let input = RtpTimestampInput {
            pts: 9_000,
            dts: 9_000,
            timebase: Timebase::new(1, 90_000),
            media_kind: MediaKind::Video,
            codec: CodecId::H264,
            clock_rate: 0,
            mode: RtpTimestampMode::Live,
            source_frame_number: None,
            source_pts: None,
            source_timebase: None,
            samples_per_frame: None,
        };
        assert_eq!(compute_rtp_timestamp(&input), 0);
    }
}
