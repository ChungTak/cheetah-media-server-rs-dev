use std::collections::VecDeque;

use bytes::Bytes;

/// A single TS segment stored in memory.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Segment name (without .ts extension), used as lookup key.
    pub name: String,
    /// Segment duration in seconds.
    pub duration_secs: f64,
    /// TS payload bytes.
    pub data: Bytes,
    /// Whether this segment starts with a keyframe.
    pub starts_with_keyframe: bool,
    /// Sequence number assigned by the ring.
    pub sequence: u64,
    /// Absolute wall-clock time (Unix millis) when segment starts. Used for EXT-X-PROGRAM-DATE-TIME.
    pub program_date_time_ms: Option<i64>,
    /// Pre-formatted CUE tags to output before this segment in playlist.
    pub cue_tags: Option<String>,
}

/// Bounded ring buffer of TS segments for a single stream.
///
/// `max_segments` controls the live playlist window size.
/// `retain_extra` keeps additional segments beyond the window for slow clients.
/// Total capacity = max_segments + retain_extra.
pub struct SegmentRing {
    segments: VecDeque<Segment>,
    max_segments: usize,
    retain_extra: usize,
    next_sequence: u64,
}

impl SegmentRing {
    pub fn new(max_segments: usize) -> Self {
        Self::with_retain(max_segments, 0)
    }

    /// Create a ring with extra retention beyond the live window.
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
    pub fn push(
        &mut self,
        name: String,
        duration_secs: f64,
        data: Bytes,
        keyframe: bool,
    ) -> Option<Segment> {
        self.push_with_pdt(name, duration_secs, data, keyframe, None)
    }

    /// Push a completed segment with optional PROGRAM-DATE-TIME.
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
    pub fn get(&self, name: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.name == name)
    }

    /// The media sequence number of the first segment in the ring.
    pub fn first_sequence(&self) -> u64 {
        self.segments.front().map(|s| s.sequence).unwrap_or(0)
    }

    /// Number of segments currently stored.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Iterator over all segments in order (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &Segment> {
        self.segments.iter()
    }

    /// Iterator over only the live window (most recent max_segments).
    pub fn live_window_iter(&self) -> impl Iterator<Item = &Segment> {
        let skip = self.segments.len().saturating_sub(self.max_segments);
        self.segments.iter().skip(skip)
    }

    /// The media sequence of the first segment in the live window.
    pub fn live_window_first_sequence(&self) -> u64 {
        let skip = self.segments.len().saturating_sub(self.max_segments);
        self.segments.get(skip).map(|s| s.sequence).unwrap_or(0)
    }

    /// Maximum segment duration across all stored segments.
    pub fn max_duration(&self) -> f64 {
        self.segments
            .iter()
            .map(|s| s.duration_secs)
            .fold(0.0_f64, f64::max)
    }

    /// Get the most recently added segment.
    pub fn latest(&self) -> Option<&Segment> {
        self.segments.back()
    }

    /// Get the most recently added segment (mutable).
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
