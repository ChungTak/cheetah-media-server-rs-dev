//! In-memory segment ring buffer for a single HLS stream.
//!
//! 单个 HLS 流的内存分片环形缓冲。
//! 维护最近分片，支持直播窗口与额外保留分片，用于慢客户端。

use std::collections::VecDeque;

use bytes::Bytes;

/// A single TS/fMP4 segment stored in memory.
///
/// 内存中存储的单个 TS/fMP4 分片。
#[derive(Debug, Clone)]
pub struct Segment {
    /// Segment name (without extension), used as lookup key.
    ///
    /// 分片名称（无扩展名），用作查找键。
    pub name: String,
    /// Segment duration in seconds.
    ///
    /// 分片时长（秒）。
    pub duration_secs: f64,
    /// Segment payload bytes.
    ///
    /// 分片负载字节。
    pub data: Bytes,
    /// Whether this segment starts with a keyframe.
    ///
    /// 分片是否以关键帧开始。
    pub starts_with_keyframe: bool,
    /// Sequence number assigned by the ring.
    ///
    /// 环形缓冲分配的分片序列号。
    pub sequence: u64,
    /// Absolute wall-clock time (Unix millis) when the segment starts. Used for `EXT-X-PROGRAM-DATE-TIME`.
    ///
    /// 分片开始时的绝对墙上时间（Unix 毫秒）。用于 `EXT-X-PROGRAM-DATE-TIME`。
    pub program_date_time_ms: Option<i64>,
    /// Pre-formatted CUE tags to output before this segment in the playlist.
    ///
    /// 在播放列表中该分片前输出的预格式化 CUE 标签。
    pub cue_tags: Option<String>,
}

/// Bounded ring buffer of segments for a single stream.
///
/// 单个流的有界分片环形缓冲。
///
/// `max_segments` controls the live playlist window size.
/// `retain_extra` keeps additional segments beyond the window for slow clients.
/// Total capacity = `max_segments + retain_extra`.
///
/// `max_segments` 控制直播播放列表窗口大小；
/// `retain_extra` 保留窗口之外的额外分片，供慢客户端使用。
/// 总容量 = `max_segments + retain_extra`。
pub struct SegmentRing {
    segments: VecDeque<Segment>,
    max_segments: usize,
    retain_extra: usize,
    next_sequence: u64,
}

impl SegmentRing {
    /// Create a ring with the given live window size.
    ///
    /// 使用给定的直播窗口大小创建环形缓冲。
    pub fn new(max_segments: usize) -> Self {
        Self::with_retain(max_segments, 0)
    }

    /// Create a ring with extra retention beyond the live window.
    ///
    /// 创建带额外保留的环形缓冲。
    pub fn with_retain(max_segments: usize, retain_extra: usize) -> Self {
        let total = max_segments + retain_extra;
        Self {
            segments: VecDeque::with_capacity(total + 1),
            max_segments: max_segments.max(1),
            retain_extra,
            next_sequence: 0,
        }
    }

    /// Push a completed segment. Returns the evicted segment if total capacity exceeded.
    ///
    /// 推入一个已完成分片。若超出总容量，返回被逐出的最旧分片。
    pub fn push(
        &mut self,
        name: String,
        duration_secs: f64,
        data: Bytes,
        keyframe: bool,
    ) -> Option<Segment> {
        self.push_with_pdt(name, duration_secs, data, keyframe, None)
    }

    /// Push a completed segment with optional `PROGRAM-DATE-TIME`.
    ///
    /// 推入带可选 `PROGRAM-DATE-TIME` 的已完成分片。
    pub fn push_with_pdt(
        &mut self,
        name: String,
        duration_secs: f64,
        data: Bytes,
        keyframe: bool,
        program_date_time_ms: Option<i64>,
    ) -> Option<Segment> {
        let seq = self.next_sequence;
        self.next_sequence += 1;

        self.segments.push_back(Segment {
            name,
            duration_secs,
            data,
            starts_with_keyframe: keyframe,
            sequence: seq,
            program_date_time_ms,
            cue_tags: None,
        });

        let total_capacity = self.max_segments + self.retain_extra;
        if self.segments.len() > total_capacity {
            self.segments.pop_front()
        } else {
            None
        }
    }

    /// Look up a segment by name.
    ///
    /// 按名称查找分片。
    pub fn get(&self, name: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.name == name)
    }

    /// The media sequence number of the first segment in the ring.
    ///
    /// 环形缓冲中第一个分片的媒体序列号。
    pub fn first_sequence(&self) -> u64 {
        self.segments.front().map(|s| s.sequence).unwrap_or(0)
    }

    /// Number of segments currently stored.
    ///
    /// 当前存储的分片数量。
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether the ring contains no segments.
    ///
    /// 环形缓冲是否为空。
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Iterator over all segments in order (oldest first).
    ///
    /// 按顺序遍历所有分片（最旧在前）。
    pub fn iter(&self) -> impl Iterator<Item = &Segment> {
        self.segments.iter()
    }

    /// Iterator over only the live window (most recent `max_segments`).
    ///
    /// 仅遍历直播窗口（最近的 `max_segments` 个分片）。
    pub fn live_window_iter(&self) -> impl Iterator<Item = &Segment> {
        let skip = self.segments.len().saturating_sub(self.max_segments);
        self.segments.iter().skip(skip)
    }

    /// The media sequence of the first segment in the live window.
    ///
    /// 直播窗口中第一个分片的媒体序列号。
    pub fn live_window_first_sequence(&self) -> u64 {
        let skip = self.segments.len().saturating_sub(self.max_segments);
        self.segments.get(skip).map(|s| s.sequence).unwrap_or(0)
    }

    /// Maximum segment duration across all stored segments.
    ///
    /// 所有存储分片中的最大时长。
    pub fn max_duration(&self) -> f64 {
        self.segments
            .iter()
            .map(|s| s.duration_secs)
            .fold(0.0_f64, f64::max)
    }

    /// Get the most recently added segment.
    ///
    /// 获取最近添加的分片。
    pub fn latest(&self) -> Option<&Segment> {
        self.segments.back()
    }

    /// Get the most recently added segment (mutable).
    ///
    /// 获取最近添加的分片（可变）。
    pub fn latest_mut(&mut self) -> Option<&mut Segment> {
        self.segments.back_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_evicts_oldest() {
        let mut ring = SegmentRing::new(3);
        ring.push("s0".into(), 2.0, Bytes::from_static(b"a"), true);
        ring.push("s1".into(), 2.0, Bytes::from_static(b"b"), true);
        ring.push("s2".into(), 2.0, Bytes::from_static(b"c"), true);
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.first_sequence(), 0);

        let evicted = ring.push("s3".into(), 2.0, Bytes::from_static(b"d"), true);
        assert_eq!(evicted.unwrap().name, "s0");
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.first_sequence(), 1);
        assert!(ring.get("s0").is_none());
        assert!(ring.get("s3").is_some());
    }
}
