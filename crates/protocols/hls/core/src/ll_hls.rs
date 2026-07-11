//! Low-Latency HLS (LL-HLS) support.
//!
//! 低延迟 HLS（LL-HLS）支持。
//! 实现 EXT-X-PART 子分段管理、切分决策逻辑以及符合 Apple LL-HLS 规范的播放列表标签生成。

use std::collections::VecDeque;

use bytes::Bytes;

/// Stable lane identifier for demuxed per-track LLHLS.
/// Maps to a logical role (video/audio), not a physical TrackId.
///
/// 解复用（per-track）LLHLS 的稳定通道标识。
/// 映射到逻辑角色（video/audio），而非物理 TrackId。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackLane {
    Video,
    Audio,
}

impl TrackLane {
    /// URL prefix for this lane's resources.
    ///
    /// 该通道资源的 URL 前缀。
    pub fn prefix(&self) -> &'static str {
        match self {
            TrackLane::Video => "video",
            TrackLane::Audio => "audio",
        }
    }
}

/// LLHLS packaging mode configuration.
///
/// LLHLS 打包模式配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlHlsPackagingMode {
    /// Per-track demuxed audio/video (default for browser LLHLS).
    ///
    /// 按轨道分离的音频/视频（浏览器 LLHLS 默认）。
    DemuxedAv,
    /// Video-only workaround (legacy, skips audio in LLHLS fMP4).
    ///
    /// 仅视频兼容模式（旧版，在 LLHLS fMP4 中跳过音频）。
    VideoOnly,
    /// Muxed audio+video in single fMP4 (non-browser compat only).
    ///
    /// 单 fMP4 中复用音频+视频（仅非浏览器兼容）。
    Muxed,
}

impl LlHlsPackagingMode {
    /// Parse packaging mode from a string, defaulting to `DemuxedAv`.
    ///
    /// 从字符串解析打包模式，默认返回 `DemuxedAv`。
    pub fn parse(s: &str) -> Self {
        match s {
            "video-only" => Self::VideoOnly,
            "muxed" => Self::Muxed,
            _ => Self::DemuxedAv,
        }
    }
}

/// A partial segment (sub-segment) for LL-HLS.
///
/// LL-HLS 的部分分段（子分段）。
#[derive(Debug, Clone)]
pub struct HlsPart {
    pub uri: String,
    pub duration_secs: f64,
    pub independent: bool,
    pub data: Bytes,
    /// Global part sequence number.
    ///
    /// 全局 part 序列号。
    pub sequence: u64,
    /// Parent segment sequence number.
    ///
    /// 所属父分段的序列号。
    pub segment_sequence: u64,
}

/// Completed segment's parts snapshot (archived when segment finalizes).
///
/// 已完成分段的 parts 快照（分段完成时归档）。
#[derive(Debug, Clone)]
pub struct SegmentParts {
    pub segment_sequence: u64,
    pub parts: Vec<HlsPart>,
}

/// LL-HLS state for a single stream.
///
/// 单个流的 LL-HLS 状态。
///
/// Holds the currently accumulating part, a ring of completed segment parts, and
/// counters used for `#EXT-X-PART` and `#EXT-X-PRELOAD-HINT` generation.
///
/// 保存当前正在累积的 part、已完成分段 parts 的环形缓冲，以及用于生成
/// `#EXT-X-PART` 和 `#EXT-X-PRELOAD-HINT` 的计数器。
pub struct LowLatencyState {
    /// Parts of the current (in-progress) segment.
    ///
    /// 当前（进行中）分段的 parts。
    current_parts: Vec<HlsPart>,
    /// Archived parts from completed segments (ring buffer).
    ///
    /// 已完成分段归档的 parts（环形缓冲）。
    completed_segments_parts: VecDeque<SegmentParts>,
    /// Maximum number of completed segment part-lists to retain.
    ///
    /// 保留的已完成分段 part 列表最大数量。
    max_completed_segments: usize,
    part_target_secs: f64,
    /// Global part sequence counter.
    ///
    /// 全局 part 序列计数器。
    part_seq: u64,
    /// Current in-progress segment sequence.
    ///
    /// 当前进行中分段的序列号。
    parent_segment_seq: u64,
    /// DTS (ms) of the first sample in the current part accumulation.
    ///
    /// 当前 part 累积中第一个 sample 的 DTS（毫秒）。
    current_part_start_dts_ms: Option<u64>,
    /// Whether the current part accumulation contains a keyframe.
    ///
    /// 当前 part 累积是否包含关键帧。
    current_part_has_keyframe: bool,
    /// Rendition reports for other tracks (populated by module layer for ABR).
    ///
    /// 其他轨道的 rendition 报告（由模块层填充，用于 ABR）。
    rendition_reports: Vec<RenditionReport>,
}

/// Info about another rendition for `EXT-X-RENDITION-REPORT`.
///
/// 用于 `EXT-X-RENDITION-REPORT` 的其他 rendition 信息。
#[derive(Debug, Clone)]
pub struct RenditionReport {
    pub uri: String,
    pub last_msn: u64,
    pub last_part: u64,
}

impl LowLatencyState {
    /// Create a new LL-HLS state with the given part target duration and archive limit.
    ///
    /// 使用给定的 part 目标时长与归档上限创建新的 LL-HLS 状态。
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
    ///
    /// 更新 part 目标时长（例如在帧对齐重新计算后）。
    pub fn set_part_target_ms(&mut self, ms: u64) {
        self.part_target_secs = ms as f64 / 1000.0;
    }

    /// Check if a part cut should happen based on accumulated duration.
    ///
    /// A cut is triggered once the difference between `sample_dts_ms` and the first
    /// sample of the current part reaches the target duration.
    ///
    /// 根据累积时长判断是否应该切分一个 part。
    /// 当 `sample_dts_ms` 与当前 part 第一个 sample 的差值达到目标时长时触发切分。
    pub fn should_cut_part(&self, sample_dts_ms: u64) -> bool {
        let Some(start) = self.current_part_start_dts_ms else {
            return false;
        };
        let elapsed_ms = sample_dts_ms.saturating_sub(start);
        let target_ms = (self.part_target_secs * 1000.0) as u64;
        elapsed_ms >= target_ms
    }

    /// Mark that a new sample is being accumulated for the current part.
    ///
    /// The first sample of a part determines independence: `INDEPENDENT=YES` is
    /// only set when the first sample is a keyframe (per Apple LLHLS spec).
    ///
    /// 标记新 sample 被累积到当前 part。
    /// part 的第一个 sample 决定独立性：仅当第一个 sample 为关键帧时设置 `INDEPENDENT=YES`（遵循 Apple LLHLS 规范）。
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
    ///
    /// Assigns the next global part sequence number, archives the part in the current
    /// segment list, and resets the accumulation state.
    ///
    /// 使用给定的 fMP4 数据完成当前 part。返回已完成的 part。
    /// 分配下一个全局 part 序列号，将 part 归档到当前分段列表，并重置累积状态。
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
    ///
    /// If the current segment has produced parts, they are moved into the completed
    /// ring. The oldest entry is dropped when the archive limit is exceeded.
    ///
    /// 分段边界到达时调用。归档当前 parts 并重置状态。
    /// 若当前分段已生成 parts，则将其移入完成环形缓冲；超过归档上限时丢弃最旧条目。
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
    ///
    /// Searches the current in-progress segment first, then completed segments.
    ///
    /// 按全局序列号查找 part。
    /// 先搜索当前进行中的分段，再搜索已完成分段。
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
    ///
    /// 当前活动（进行中）分段的 parts。
    pub fn current_parts(&self) -> &[HlsPart] {
        &self.current_parts
    }

    /// Archived segment parts (completed segments).
    ///
    /// 已归档的分段 parts（已完成分段）。
    pub fn completed_segments_parts(&self) -> &VecDeque<SegmentParts> {
        &self.completed_segments_parts
    }

    /// Part target duration in seconds.
    ///
    /// part 目标时长（秒）。
    pub fn part_target(&self) -> f64 {
        self.part_target_secs
    }

    /// Current parent segment sequence.
    ///
    /// 当前父分段序列号。
    pub fn parent_segment_seq(&self) -> u64 {
        self.parent_segment_seq
    }

    /// Global part sequence (next part will get this number).
    ///
    /// 全局 part 序列号（下一个 part 将使用此编号）。
    pub fn next_part_seq(&self) -> u64 {
        self.part_seq
    }

    /// Generate LL-HLS playlist header tags (`SERVER-CONTROL` + `PART-INF`).
    ///
    /// `PART-HOLD-BACK` is set to three times the part target to hint the player
    /// how far behind the live edge it should stay.
    ///
    /// 生成 LL-HLS 播放列表头部标签（`SERVER-CONTROL` + `PART-INF`）。
    /// `PART-HOLD-BACK` 设置为 part 目标时长的三倍，提示播放器应距离直播边缘多远。
    pub fn playlist_header_tags(&self) -> String {
        let part_hold_back = self.part_target_secs * 3.0;
        format!(
            "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK={part_hold_back:.1}\n\
             #EXT-X-PART-INF:PART-TARGET={:.3}\n",
            self.part_target_secs
        )
    }

    /// Generate `#EXT-X-PART` tags for a given list of parts.
    ///
    /// 为给定的 part 列表生成 `#EXT-X-PART` 标签。
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

    /// Generate `#EXT-X-PART` tags for current (in-progress) segment parts.
    ///
    /// 为当前（进行中）分段 parts 生成 `#EXT-X-PART` 标签。
    pub fn part_tags(&self, prefix: &str) -> String {
        Self::format_part_tags(&self.current_parts, prefix)
    }

    /// Generate `#EXT-X-PRELOAD-HINT` tag for the next expected part.
    ///
    /// 生成下一个预期 part 的 `#EXT-X-PRELOAD-HINT` 标签。
    pub fn preload_hint_tag(&self, prefix: &str) -> String {
        format!(
            "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"{prefix}part_{}.m4s\"\n",
            self.part_seq
        )
    }

    /// Set rendition reports (other tracks' state for ABR).
    ///
    /// 设置 rendition 报告（其他轨道状态，用于 ABR）。
    pub fn set_rendition_reports(&mut self, reports: Vec<RenditionReport>) {
        self.rendition_reports = reports;
    }

    /// Generate `#EXT-X-RENDITION-REPORT` tags for all other renditions.
    ///
    /// 为所有其他 rendition 生成 `#EXT-X-RENDITION-REPORT` 标签。
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
