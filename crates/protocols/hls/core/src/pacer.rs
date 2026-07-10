//! HLS playback pacing: buffers demuxed frames and releases them at real-time rate.
//!
//! Used by the HLS player to convert bursty segment downloads into smooth frame delivery.

use std::collections::VecDeque;

/// A frame waiting to be paced out.
#[derive(Debug, Clone)]
pub struct PacedFrame {
    pub dts_ms: u64,
    pub data: bytes::Bytes,
    pub media_kind: cheetah_codec::MediaKind,
    pub codec: cheetah_codec::CodecId,
    pub pts_ms: u64,
    pub keyframe: bool,
}

/// Buffers frames and releases them at real-time pace.
///
/// Call `push()` when frames arrive from segment download.
/// Call `drain_ready()` periodically (every ~50ms) to get frames that should play now.
pub struct HlsPlaybackPacer {
    buffer: VecDeque<PacedFrame>,
    /// Wall-clock time (micros) when playback started.
    play_start_us: Option<u64>,
    /// DTS (ms) of the first frame, used as time base.
    first_dts_ms: Option<u64>,
    /// Maximum buffer before force-drain (ms).
    max_buffer_ms: u64,
}

impl HlsPlaybackPacer {
    /// Creates a new `HlsPlaybackPacer` instance.
    /// 创建新的 `HlsPlaybackPacer` 实例。
    pub fn new(max_buffer_ms: u64) -> Self {
        Self {
            buffer: VecDeque::new(),
            play_start_us: None,
            first_dts_ms: None,
            max_buffer_ms,
        }
    }

    /// Add a frame to the buffer.
    pub fn push(&mut self, frame: PacedFrame) {
        if self.first_dts_ms.is_none() {
            self.first_dts_ms = Some(frame.dts_ms);
        }
        self.buffer.push_back(frame);
    }

    /// Drain frames that should be played by `now_us` (wall-clock micros).
    /// Returns frames in DTS order.
    pub fn drain_ready(&mut self, now_us: u64) -> Vec<PacedFrame> {
        let play_start = match self.play_start_us {
            Some(t) => t,
            None => {
                if self.buffer.is_empty() {
                    return Vec::new();
                }
                self.play_start_us = Some(now_us);
                now_us
            }
        };

        let first_dts = match self.first_dts_ms {
            Some(d) => d,
            None => return Vec::new(),
        };

        // Elapsed wall-clock time since playback started (in ms)
        let elapsed_ms = (now_us.saturating_sub(play_start)) / 1000;

        // Force drain if buffer is too large
        let buffer_span_ms = self
            .buffer
            .back()
            .map(|b| b.dts_ms.saturating_sub(first_dts))
            .unwrap_or(0);
        let force_drain = buffer_span_ms > self.max_buffer_ms;

        let mut out = Vec::with_capacity(self.buffer.len());
        while let Some(front) = self.buffer.front() {
            let frame_offset_ms = front.dts_ms.saturating_sub(first_dts);
            if frame_offset_ms <= elapsed_ms || force_drain {
                out.push(self.buffer.pop_front().unwrap());
                // In force-drain mode, only drain down to half max buffer
                if force_drain && !out.is_empty() {
                    let remaining_span = self
                        .buffer
                        .back()
                        .map(|b| b.dts_ms.saturating_sub(first_dts))
                        .unwrap_or(0);
                    if remaining_span <= self.max_buffer_ms / 2 {
                        break;
                    }
                }
            } else {
                break;
            }
        }
        out
    }

    /// Number of buffered frames.
    pub fn buffered_count(&self) -> usize {
        self.buffer.len()
    }

    /// Reset the pacer state.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.play_start_us = None;
        self.first_dts_ms = None;
    }
}

/// Timestamp smoother: handles discontinuities, jumps, and non-monotonic timestamps.
///
/// Input timestamps may jump forward/backward due to encoder restarts, segment
/// boundaries, or stream switching. The smoother maintains continuity in output.
pub struct StampSmoother {
    last_input: i64,
    last_output: i64,
    offset: i64,
    max_forward_jump_ms: i64,
    max_backward_jump_ms: i64,
    started: bool,
}

impl StampSmoother {
    /// Creates a new `StampSmoother` instance.
    /// 创建新的 `StampSmoother` 实例。
    pub fn new(max_forward_jump_ms: i64, max_backward_jump_ms: i64) -> Self {
        Self {
            last_input: 0,
            last_output: 0,
            offset: 0,
            max_forward_jump_ms,
            max_backward_jump_ms,
            started: false,
        }
    }

    /// Smooth an input timestamp (ms), returning a corrected output timestamp.
    pub fn smooth(&mut self, input_ms: i64) -> i64 {
        if !self.started {
            self.started = true;
            self.last_input = input_ms;
            self.last_output = 0;
            self.offset = -input_ms;
            return 0;
        }

        let delta = input_ms - self.last_input;

        if delta > self.max_forward_jump_ms || delta < -self.max_backward_jump_ms {
            // Discontinuity: adjust offset to maintain output continuity
            let estimated_duration = 33; // ~30fps fallback
            self.offset = self.last_output + estimated_duration - input_ms;
        }

        self.last_input = input_ms;
        self.last_output = (input_ms + self.offset).max(0);
        self.last_output
    }

    /// Reset the smoother state.
    pub fn reset(&mut self) {
        self.started = false;
        self.last_input = 0;
        self.last_output = 0;
        self.offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use cheetah_codec::{CodecId, MediaKind};

    fn make_frame(dts_ms: u64) -> PacedFrame {
        PacedFrame {
            dts_ms,
            pts_ms: dts_ms,
            data: Bytes::from_static(b"x"),
            media_kind: MediaKind::Video,
            codec: CodecId::H264,
            keyframe: false,
        }
    }

    #[test]
    fn paces_frames_by_dts() {
        let mut pacer = HlsPlaybackPacer::new(30000);
        pacer.push(make_frame(0));
        pacer.push(make_frame(40));
        pacer.push(make_frame(80));

        // At t=0, only first frame should be ready
        let start = 1_000_000_u64; // 1s in micros
        let out = pacer.drain_ready(start);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dts_ms, 0);

        // At t=50ms, second frame should be ready
        let out = pacer.drain_ready(start + 50_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dts_ms, 40);

        // At t=100ms, third frame should be ready
        let out = pacer.drain_ready(start + 100_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dts_ms, 80);
    }

    #[test]
    fn force_drains_on_overflow() {
        let mut pacer = HlsPlaybackPacer::new(100); // 100ms max buffer
        for i in 0..50 {
            pacer.push(make_frame(i * 40)); // 2s of frames
        }

        let start = 1_000_000_u64;
        let out = pacer.drain_ready(start);
        // Should force-drain since buffer > 100ms
        assert!(!out.is_empty());
    }
}
