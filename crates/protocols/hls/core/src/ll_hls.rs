//! Low-Latency HLS (LL-HLS) support.
//!
//! Implements EXT-X-PART sub-segment management, cut decision logic, and playlist
//! tag generation per Apple's Low-Latency HLS specification.

use std::collections::VecDeque;

use bytes::Bytes;

/// Stable lane identifier for demuxed per-track LLHLS.
/// Maps to a logical role (video/audio), not a physical TrackId.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackLane {
    /// `Video` variant.
    /// `Video` 变体.
    Video,
    /// `Audio` variant.
    /// `Audio` 变体.
    Audio,
}

impl TrackLane {
    /// URL prefix for this lane's resources.
    pub fn prefix(&self) -> &'static str {
        match self {
            TrackLane::Video => "video",
            TrackLane::Audio => "audio",
        }
    }
}

/// LLHLS packaging mode configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlHlsPackagingMode {
    /// Per-track demuxed audio/video (default for browser LLHLS).
    DemuxedAv,
    /// Video-only workaround (legacy, skips audio in LLHLS fMP4).
    VideoOnly,
    /// Muxed audio+video in single fMP4 (non-browser compat only).
    Muxed,
}

impl LlHlsPackagingMode {
    /// `parse` function.
    /// `parse` 函数.
    pub fn parse(s: &str) -> Self {
        match s {
            "video-only" => Self::VideoOnly,
            "muxed" => Self::Muxed,
            _ => Self::DemuxedAv,
        }
    }
}

/// A partial segment (sub-segment) for LL-HLS.
#[derive(Debug, Clone)]
pub struct HlsPart {
    /// `uri` field of type `String`.
    /// `uri` 字段，类型为 `String`.
    pub uri: String,
    /// `duration_secs` field of type `f64`.
    /// `duration_secs` 字段，类型为 `f64`.
    pub duration_secs: f64,
    /// `independent` field of type `bool`.
    /// `independent` 字段，类型为 `bool`.
    pub independent: bool,
    /// `data` field of type `Bytes`.
    /// `data` 字段，类型为 `Bytes`.
    pub data: Bytes,
    /// Global part sequence number.
    pub sequence: u64,
    /// Parent segment sequence number.
    pub segment_sequence: u64,
}

/// Completed segment's parts snapshot (archived when segment finalizes).
#[derive(Debug, Clone)]
pub struct SegmentParts {
    /// `segment_sequence` field of type `u64`.
    /// `segment_sequence` 字段，类型为 `u64`.
    pub segment_sequence: u64,
    /// `parts` field.
    /// `parts` 字段.
    pub parts: Vec<HlsPart>,
}

/// LL-HLS state for a single stream.
pub struct LowLatencyState {
    /// Parts of the current (in-progress) segment.
    current_parts: Vec<HlsPart>,
    /// Archived parts from completed segments (ring buffer).
    completed_segments_parts: VecDeque<SegmentParts>,
    /// Maximum number of completed segment part-lists to retain.
    max_completed_segments: usize,
    /// `part_target_secs` field of type `f64`.
    /// `part_target_secs` 字段，类型为 `f64`.
    part_target_secs: f64,
    /// Global part sequence counter.
    part_seq: u64,
    /// Current in-progress segment sequence.
    parent_segment_seq: u64,
    /// DTS (ms) of the first sample in the current part accumulation.
    current_part_start_dts_ms: Option<u64>,
    /// Whether the current part accumulation contains a keyframe.
    current_part_has_keyframe: bool,
    /// Rendition reports for other tracks (populated by module layer for ABR).
    rendition_reports: Vec<RenditionReport>,
}

/// Info about another rendition for EXT-X-RENDITION-REPORT.
#[derive(Debug, Clone)]
pub struct RenditionReport {
    /// `uri` field of type `String`.
    /// `uri` 字段，类型为 `String`.
    pub uri: String,
    /// `last_msn` field of type `u64`.
    /// `last_msn` 字段，类型为 `u64`.
    pub last_msn: u64,
    /// `last_part` field of type `u64`.
    /// `last_part` 字段，类型为 `u64`.
    pub last_part: u64,
}

impl LowLatencyState {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(part_target_ms: u64, max_completed_segments: usize) -> Self {
        Self {
            current_parts: Vec::new(),
            completed_segments_parts: VecDeque::new(),
            max_completed_segments,
            part_target_secs: part_target_ms as f64 / 1000.0,
            part_seq: 0,
            parent_segment_seq: 0,
            current_part_start_dts_ms: None,
            current_part_has_keyframe: false,
            rendition_reports: Vec::new(),
        }
    }

    /// Update part target duration (e.g., after frame-aligned recalculation).
    pub fn set_part_target_ms(&mut self, ms: u64) {
        self.part_target_secs = ms as f64 / 1000.0;
    }

    /// Check if a part cut should happen based on accumulated duration.
    pub fn should_cut_part(&self, sample_dts_ms: u64) -> bool {
        let Some(start) = self.current_part_start_dts_ms else {
            return false;
        };
        let elapsed_ms = sample_dts_ms.saturating_sub(start);
        let target_ms = (self.part_target_secs * 1000.0) as u64;
        elapsed_ms >= target_ms
    }

    /// Mark that a new sample is being accumulated for the current part.
    pub fn note_sample(&mut self, dts_ms: u64, is_keyframe: bool) {
        if self.current_part_start_dts_ms.is_none() {
            self.current_part_start_dts_ms = Some(dts_ms);
            // INDEPENDENT=YES only if the FIRST sample of the part is a keyframe
            // (per Apple LLHLS spec / OME behavior)
            if is_keyframe {
                self.current_part_has_keyframe = true;
            }
        }
    }

    /// Finalize the current part with the given fMP4 data. Returns the completed part.
    pub fn finalize_part(&mut self, data: Bytes, duration_secs: f64) -> HlsPart {
        let seq = self.part_seq;
        self.part_seq += 1;
        let independent = self.current_part_has_keyframe;
        let part = HlsPart {
            uri: format!("part_{seq}.m4s"),
            duration_secs,
            independent,
            data,
            sequence: seq,
            segment_sequence: self.parent_segment_seq,
        };
        self.current_parts.push(part.clone());
        // Reset accumulation state
        self.current_part_start_dts_ms = None;
        self.current_part_has_keyframe = false;
        part
    }

    /// Called when a segment boundary is reached. Archives current parts and resets.
    pub fn on_segment_boundary(&mut self, new_segment_seq: u64) {
        if !self.current_parts.is_empty() {
            let archived = SegmentParts {
                segment_sequence: self.parent_segment_seq,
                parts: std::mem::take(&mut self.current_parts),
            };
            self.completed_segments_parts.push_back(archived);
            if self.completed_segments_parts.len() > self.max_completed_segments {
                self.completed_segments_parts.pop_front();
            }
        }
        self.parent_segment_seq = new_segment_seq;
        self.current_part_start_dts_ms = None;
        self.current_part_has_keyframe = false;
    }

    /// Get a part by its global sequence number.
    pub fn get_part(&self, part_seq: u64) -> Option<&HlsPart> {
        // Search current parts
        if let Some(p) = self.current_parts.iter().find(|p| p.sequence == part_seq) {
            return Some(p);
        }
        // Search archived parts
        for sp in &self.completed_segments_parts {
            if let Some(p) = sp.parts.iter().find(|p| p.sequence == part_seq) {
                return Some(p);
            }
        }
        None
    }

    /// Current parts for the active (in-progress) segment.
    pub fn current_parts(&self) -> &[HlsPart] {
        &self.current_parts
    }

    /// Archived segment parts (completed segments).
    pub fn completed_segments_parts(&self) -> &VecDeque<SegmentParts> {
        &self.completed_segments_parts
    }

    /// Part target duration in seconds.
    pub fn part_target(&self) -> f64 {
        self.part_target_secs
    }

    /// Current parent segment sequence.
    pub fn parent_segment_seq(&self) -> u64 {
        self.parent_segment_seq
    }

    /// Global part sequence (next part will get this number).
    pub fn next_part_seq(&self) -> u64 {
        self.part_seq
    }

    /// Generate LL-HLS playlist header tags (SERVER-CONTROL + PART-INF).
    pub fn playlist_header_tags(&self) -> String {
        let part_hold_back = self.part_target_secs * 3.0;
        format!(
            "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK={part_hold_back:.1}\n\
             #EXT-X-PART-INF:PART-TARGET={:.3}\n",
            self.part_target_secs
        )
    }

    /// Generate EXT-X-PART tags for a given list of parts.
    pub fn format_part_tags(parts: &[HlsPart], prefix: &str) -> String {
        let mut out = String::new();
        for part in parts {
            out.push_str(&format!(
                "#EXT-X-PART:DURATION={:.3},URI=\"{prefix}{}\"",
                part.duration_secs, part.uri
            ));
            if part.independent {
                out.push_str(",INDEPENDENT=YES");
            }
            out.push('\n');
        }
        out
    }

    /// Generate EXT-X-PART tags for current (in-progress) segment parts.
    pub fn part_tags(&self, prefix: &str) -> String {
        Self::format_part_tags(&self.current_parts, prefix)
    }

    /// Generate EXT-X-PRELOAD-HINT tag for the next expected part.
    pub fn preload_hint_tag(&self, prefix: &str) -> String {
        format!(
            "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"{prefix}part_{}.m4s\"\n",
            self.part_seq
        )
    }

    /// Set rendition reports (other tracks' state for ABR).
    pub fn set_rendition_reports(&mut self, reports: Vec<RenditionReport>) {
        self.rendition_reports = reports;
    }

    /// Generate EXT-X-RENDITION-REPORT tags for all other renditions.
    pub fn rendition_report_tags(&self) -> String {
        let mut out = String::new();
        for r in &self.rendition_reports {
            out.push_str(&format!(
                "#EXT-X-RENDITION-REPORT:URI=\"{}\",LAST-MSN={},LAST-PART={}\n",
                r.uri, r.last_msn, r.last_part
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ll_hls_header_tags() {
        let state = LowLatencyState::new(500, 5);
        let header = state.playlist_header_tags();
        assert!(header.contains("CAN-BLOCK-RELOAD=YES"));
        assert!(header.contains("PART-HOLD-BACK=1.5"));
        assert!(header.contains("PART-TARGET=0.500"));
    }

    #[test]
    fn should_cut_part_timing() {
        let mut state = LowLatencyState::new(200, 5); // 200ms target
                                                      // No start yet
        assert!(!state.should_cut_part(100));
        // Note first sample
        state.note_sample(0, true);
        assert!(!state.should_cut_part(100));
        assert!(!state.should_cut_part(199));
        assert!(state.should_cut_part(200));
        assert!(state.should_cut_part(300));
    }

    #[test]
    fn finalize_part_resets_state() {
        let mut state = LowLatencyState::new(200, 5);
        state.note_sample(0, true);
        let part = state.finalize_part(Bytes::from_static(b"data"), 0.2);
        assert_eq!(part.sequence, 0);
        assert!(part.independent);
        assert_eq!(part.duration_secs, 0.2);
        // After finalize, should_cut returns false (no start)
        assert!(!state.should_cut_part(500));
        // Next part gets seq 1
        state.note_sample(200, false);
        let part2 = state.finalize_part(Bytes::from_static(b"data2"), 0.2);
        assert_eq!(part2.sequence, 1);
        assert!(!part2.independent);
    }

    #[test]
    fn on_segment_boundary_archives() {
        let mut state = LowLatencyState::new(200, 3);
        state.note_sample(0, true);
        state.finalize_part(Bytes::from_static(b"p0"), 0.2);
        state.note_sample(200, false);
        state.finalize_part(Bytes::from_static(b"p1"), 0.2);
        assert_eq!(state.current_parts().len(), 2);

        state.on_segment_boundary(1);
        assert_eq!(state.current_parts().len(), 0);
        assert_eq!(state.completed_segments_parts().len(), 1);
        assert_eq!(state.completed_segments_parts()[0].parts.len(), 2);
        assert_eq!(state.parent_segment_seq(), 1);
    }

    #[test]
    fn completed_segments_ring_evicts() {
        let mut state = LowLatencyState::new(200, 2); // max 2 completed
        state.note_sample(0, true);
        state.finalize_part(Bytes::from_static(b"p"), 0.2);
        state.on_segment_boundary(1);

        state.note_sample(200, true);
        state.finalize_part(Bytes::from_static(b"p"), 0.2);
        state.on_segment_boundary(2);

        state.note_sample(400, true);
        state.finalize_part(Bytes::from_static(b"p"), 0.2);
        state.on_segment_boundary(3);

        assert_eq!(state.completed_segments_parts().len(), 2);
        // Oldest (seg 0) was evicted
        assert_eq!(state.completed_segments_parts()[0].segment_sequence, 1);
    }

    #[test]
    fn get_part_searches_all() {
        let mut state = LowLatencyState::new(200, 5);
        state.note_sample(0, true);
        state.finalize_part(Bytes::from_static(b"p0"), 0.2);
        state.on_segment_boundary(1);
        state.note_sample(200, false);
        state.finalize_part(Bytes::from_static(b"p1"), 0.2);

        // Part 0 is in completed, part 1 is in current
        assert!(state.get_part(0).is_some());
        assert!(state.get_part(1).is_some());
        assert!(state.get_part(2).is_none());
    }

    #[test]
    fn part_tags_format() {
        let mut state = LowLatencyState::new(200, 5);
        state.note_sample(0, true);
        state.finalize_part(Bytes::from_static(b"d"), 0.2);
        state.note_sample(200, false);
        state.finalize_part(Bytes::from_static(b"d"), 0.2);

        let tags = state.part_tags("");
        assert!(tags.contains("#EXT-X-PART:DURATION=0.200,URI=\"part_0.m4s\",INDEPENDENT=YES"));
        assert!(tags.contains("#EXT-X-PART:DURATION=0.200,URI=\"part_1.m4s\""));
        assert!(!tags.contains("part_1.m4s\",INDEPENDENT"));
    }

    #[test]
    fn preload_hint_tag() {
        let mut state = LowLatencyState::new(200, 5);
        state.note_sample(0, true);
        state.finalize_part(Bytes::from_static(b"d"), 0.2);
        let hint = state.preload_hint_tag("");
        assert_eq!(hint, "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part_1.m4s\"\n");
    }
}
