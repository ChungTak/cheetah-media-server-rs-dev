//! Pure Sans-I/O WebVTT segment muxer for HLS subtitles.
//!
//! `VttMux` consumes normalized [`WebVttCue`]s and video segment boundaries,
//! emits aligned `.vtt` segments, and keeps a bounded ring of recent segments
//! for live playlist generation. Cues that cross a segment boundary are split
//! and written into every segment during which they are visible so that
//! playback never drops a caption.
//!
//! 纯无 I/O 的 HLS 字幕 WebVTT 复用器。

use std::collections::VecDeque;

use cheetah_codec::subtitle::WebVttCue;

use crate::HlsCoreError;

/// Maximum number of cues retained before being written to a segment.
const MAX_PENDING_CUES: usize = 256;

/// Maximum number of VTT segments kept in the live ring.
const DEFAULT_MAX_SEGMENTS: usize = 32;

/// Default target segment duration in milliseconds.
const DEFAULT_SEGMENT_DURATION_MS: u64 = 4_000;

/// One produced WebVTT segment.
#[derive(Debug, Clone, PartialEq)]
pub struct VttSegment {
    /// Segment name (without `.vtt` extension), used as lookup key.
    pub name: String,
    /// Monotonic sequence number.
    pub sequence: u64,
    /// Absolute presentation start time, in milliseconds.
    pub start_ms: u64,
    /// Absolute presentation end time, in milliseconds.
    pub end_ms: u64,
    /// Segment duration in seconds.
    pub duration_secs: f64,
    /// Full segment payload, including the `WEBVTT` header.
    pub payload: String,
}

/// Configuration for [`VttMux`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VttMuxConfig {
    /// Target segment duration in milliseconds.
    pub segment_duration_ms: u64,
    /// Maximum number of completed segments to retain.
    pub max_segments: usize,
}

impl Default for VttMuxConfig {
    fn default() -> Self {
        Self {
            segment_duration_ms: DEFAULT_SEGMENT_DURATION_MS,
            max_segments: DEFAULT_MAX_SEGMENTS,
        }
    }
}

/// State machine that turns a stream of [`WebVttCue`]s into HLS WebVTT segments.
#[derive(Debug, Clone)]
pub struct VttMux {
    config: VttMuxConfig,
    segments: VecDeque<VttSegment>,
    pending_cues: Vec<WebVttCue>,
    next_sequence: u64,
    next_segment_start: Option<u64>,
}

impl VttMux {
    /// Creates a new muxer with the given configuration.
    pub fn new(config: VttMuxConfig) -> Self {
        Self {
            config,
            segments: VecDeque::with_capacity(config.max_segments + 1),
            pending_cues: Vec::new(),
            next_sequence: 0,
            next_segment_start: Some(0),
        }
    }

    /// Adds a cue to the pending queue.
    ///
    /// Cues are held until a segment boundary closes. Cues with `end_ms <= start_ms`
    /// are ignored.
    pub fn push_cue(&mut self, cue: WebVttCue) -> Result<(), HlsCoreError> {
        if cue.end_ms <= cue.start_ms {
            return Err(HlsCoreError::InvalidTimestamp);
        }
        if self.pending_cues.len() >= MAX_PENDING_CUES {
            self.pending_cues.remove(0);
        }
        self.pending_cues.push(cue);
        self.pending_cues.sort_by_key(|c| c.start_ms);
        Ok(())
    }

    /// Closes the current segment at `end_ms` and starts a new one.
    ///
    /// The segment start is taken from the previous boundary, or `0` for the
    /// first segment. Cues overlapping the interval are written into the segment
    /// with absolute timestamps continuous with the media timeline; cues that
    /// continue past the end are retained for the next segment.
    pub fn close_segment(&mut self, end_ms: u64) -> Result<(), HlsCoreError> {
        let start_ms = self
            .next_segment_start
            .unwrap_or_else(|| end_ms.saturating_sub(self.config.segment_duration_ms));

        if end_ms <= start_ms {
            return Err(HlsCoreError::InvalidTimestamp);
        }

        let duration_ms = end_ms - start_ms;
        let mut payload = String::with_capacity(256);
        payload.push_str("WEBVTT\n\n");

        let mut retained = Vec::with_capacity(self.pending_cues.len());
        for cue in &self.pending_cues {
            if cue.end_ms <= start_ms || cue.start_ms >= end_ms {
                // Cue falls entirely outside this segment; keep it if it is
                // still in the future, drop it if it is in the past.
                if cue.end_ms > end_ms || cue.start_ms >= end_ms {
                    retained.push(cue.clone());
                }
                continue;
            }

            let segment_start = cue.start_ms.max(start_ms);
            let segment_end = cue.end_ms.min(end_ms);
            if segment_end > segment_start {
                payload.push_str(&format_vtt_timestamp(segment_start));
                payload.push_str(" --> ");
                payload.push_str(&format_vtt_timestamp(segment_end));
                if let Some(settings) = &cue.settings {
                    payload.push(' ');
                    payload.push_str(settings);
                }
                payload.push('\n');
                payload.push_str(&cue.payload);
                payload.push_str("\n\n");
            }

            // Retain cues that extend beyond this segment.
            if cue.end_ms > end_ms {
                retained.push(cue.clone());
            }
        }

        self.pending_cues = retained;
        self.pending_cues.sort_by_key(|c| c.start_ms);

        let segment = VttSegment {
            name: format!("sub{}", self.next_sequence),
            sequence: self.next_sequence,
            start_ms,
            end_ms,
            duration_secs: duration_ms as f64 / 1000.0,
            payload,
        };
        self.next_sequence += 1;
        self.next_segment_start = Some(end_ms);

        if self.segments.len() >= self.config.max_segments {
            self.segments.pop_front();
        }
        self.segments.push_back(segment);
        Ok(())
    }

    /// Flushes all pending cues into a final segment ending at `end_ms`.
    ///
    /// If no `end_ms` is supplied, the end time is the maximum cue end time or
    /// `start_ms + segment_duration_ms`, whichever is larger.
    pub fn flush(&mut self, end_ms: Option<u64>) -> Result<(), HlsCoreError> {
        let start_ms = self.next_segment_start.unwrap_or(0);
        let fallback_end = if let Some(last) = self.pending_cues.iter().map(|c| c.end_ms).max() {
            last.max(start_ms + self.config.segment_duration_ms)
        } else {
            start_ms + self.config.segment_duration_ms
        };
        let end_ms = end_ms.unwrap_or(fallback_end);
        self.close_segment(end_ms)
    }

    /// Returns the live ring of completed segments, oldest first.
    pub fn segments(&self) -> &VecDeque<VttSegment> {
        &self.segments
    }

    /// Returns the absolute start time of the next segment, if known.
    pub fn next_segment_start_ms(&self) -> Option<u64> {
        self.next_segment_start
    }
}

fn format_vtt_timestamp(ms: u64) -> String {
    let total = ms;
    let hh = total / 3_600_000;
    let rem = total % 3_600_000;
    let mm = rem / 60_000;
    let rem = rem % 60_000;
    let ss = rem / 1_000;
    let ttt = rem % 1_000;
    format!("{hh:02}:{mm:02}:{ss:02}.{ttt:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start_ms: u64, end_ms: u64, text: &str) -> WebVttCue {
        WebVttCue {
            id: None,
            start_ms,
            end_ms,
            payload: text.to_string(),
            settings: None,
        }
    }

    #[test]
    fn empty_muxer_produces_no_segments_until_boundary() {
        let mux = VttMux::new(VttMuxConfig::default());
        assert!(mux.segments().is_empty());
    }

    #[test]
    fn closes_segment_with_local_cues() {
        let mut mux = VttMux::new(VttMuxConfig::default());
        mux.push_cue(cue(500, 3_500, "Hello")).unwrap();
        mux.close_segment(4_000).unwrap();

        let seg = mux.segments().back().unwrap();
        assert_eq!(seg.start_ms, 0);
        assert_eq!(seg.end_ms, 4_000);
        assert!(seg.payload.contains("00:00:00.500 --> 00:00:03.500"));
        assert!(seg.payload.contains("Hello"));
    }

    #[test]
    fn splits_cross_segment_cue_into_both_segments() {
        let mut mux = VttMux::new(VttMuxConfig::default());
        mux.push_cue(cue(2_000, 6_000, "Cross")).unwrap();
        mux.close_segment(4_000).unwrap();
        mux.close_segment(8_000).unwrap();

        let first = mux.segments().iter().next().unwrap();
        let second = mux.segments().iter().nth(1).unwrap();

        assert!(first.payload.contains("00:00:02.000 --> 00:00:04.000"));
        assert!(first.payload.contains("Cross"));
        assert!(second.payload.contains("00:00:04.000 --> 00:00:06.000"));
        assert!(second.payload.contains("Cross"));
    }

    #[test]
    fn ignores_zero_duration_cue() {
        let mut mux = VttMux::new(VttMuxConfig::default());
        assert!(mux.push_cue(cue(1_000, 1_000, "Bad")).is_err());
    }

    #[test]
    fn retains_cue_for_later_segment() {
        let mut mux = VttMux::new(VttMuxConfig::default());
        mux.push_cue(cue(5_000, 7_000, "Later")).unwrap();
        mux.close_segment(4_000).unwrap();
        let first = mux.segments().back().unwrap();
        assert!(!first.payload.contains("Later"));

        mux.close_segment(8_000).unwrap();
        let second = mux.segments().back().unwrap();
        assert!(second.payload.contains("00:00:05.000 --> 00:00:07.000"));
        assert!(second.payload.contains("Later"));
    }
}
