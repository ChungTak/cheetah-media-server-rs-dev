//! Per-track fMP4 packager for demuxed LLHLS.
//!
//! Each `TrackMuxer` manages a single audio or video lane: its own init segment,
//! LL-HLS parts, segments, and ring buffer. This enables independent per-track
//! chunklists required by demuxed LLHLS (OvenMediaEngine-style).

use bytes::Bytes;
use cheetah_codec::{CodecId, MediaKind, TrackId};
use cheetah_hls_core::{
    Fmp4Muxer, Fmp4Sample, Fmp4TrackDesc, HlsPart, LowLatencyState, SegmentRing, TrackLane,
};

/// Per-track fMP4 muxer state.
pub struct TrackMuxer {
    pub lane: TrackLane,
    pub source_track_id: TrackId,
    pub media_kind: MediaKind,
    pub codec: CodecId,
    fmp4_muxer: Fmp4Muxer,
    pub init_segment: Bytes,
    pub ll_state: LowLatencyState,
    pending_part_samples: Vec<Fmp4Sample>,
    pending_segment_part_data: Vec<Bytes>,
    segment_start_dts_ms: Option<u64>,
    segment_last_dts_ms: u64,
    /// Last inter-frame interval in ms (for EXTINF estimation at flush).
    last_frame_interval_ms: Option<u64>,
    prev_dts_ms: Option<u64>,
    segment_has_keyframe: bool,
    segment_seq: u64,
    pub ring: SegmentRing,
    pub concluded: bool,
    /// Part target in ms (frame-aligned).
    part_target_ms: u64,
    /// Wallclock offset for PROGRAM-DATE-TIME (set by DemuxedStreamMuxer).
    wallclock_offset_ms: Option<i64>,
}

/// Output event from a track muxer.
#[derive(Debug, Clone)]
pub enum TrackMuxerOutput {
    PartReady(HlsPart),
    SegmentReady { name: String, duration_secs: f64 },
}

impl TrackMuxer {
    /// Creates a new `TrackMuxer` instance.
    /// 创建新的 `TrackMuxer` 实例。
    pub fn new(
        lane: TrackLane,
        source_track_id: TrackId,
        desc: Fmp4TrackDesc,
        part_target_ms: u64,
        segment_count: usize,
        max_completed_segments: usize,
    ) -> Self {
        let media_kind = desc.media_kind;
        let codec = desc.codec;
        let mut fmp4_muxer = Fmp4Muxer::new(vec![desc]);
        let init_segment = fmp4_muxer.init_segment();
        Self {
            lane,
            source_track_id,
            media_kind,
            codec,
            fmp4_muxer,
            init_segment,
            ll_state: LowLatencyState::new(part_target_ms, max_completed_segments),
            pending_part_samples: Vec::new(),
            pending_segment_part_data: Vec::new(),
            segment_start_dts_ms: None,
            segment_last_dts_ms: 0,
            last_frame_interval_ms: None,
            prev_dts_ms: None,
            segment_has_keyframe: false,
            segment_seq: 0,
            ring: SegmentRing::new(segment_count),
            concluded: false,
            part_target_ms,
            wallclock_offset_ms: None,
        }
    }

    /// Push a sample into this track muxer. Returns any outputs produced.
    pub fn push_sample(
        &mut self,
        dts_ms: u64,
        pts_ms: u64,
        is_keyframe: bool,
        data: Bytes,
        segment_duration_ms: u64,
        force_segment_after_ms: u64,
    ) -> Vec<TrackMuxerOutput> {
        if self.concluded {
            return Vec::new();
        }

        let mut outputs = Vec::new();

        // Segment cut decision
        let should_cut = if let Some(start) = self.segment_start_dts_ms {
            let elapsed = dts_ms.saturating_sub(start);
            let is_video = self.media_kind == MediaKind::Video;
            let normal_cut = is_video && is_keyframe && elapsed >= segment_duration_ms;
            let force_cut = elapsed >= force_segment_after_ms;
            // Audio: cut at segment boundary without keyframe requirement
            let audio_cut = !is_video
                && elapsed >= segment_duration_ms
                && !self.pending_part_samples.is_empty();
            normal_cut || force_cut || audio_cut
        } else {
            false
        };

        if should_cut {
            if let Some(part) = self.finalize_current_part(Some(dts_ms)) {
                outputs.push(TrackMuxerOutput::PartReady(part));
            }
            if let Some(seg_out) = self.finalize_segment(Some(dts_ms)) {
                outputs.push(seg_out);
            }
        }

        if self.segment_start_dts_ms.is_none() {
            self.segment_start_dts_ms = Some(dts_ms);
            self.segment_has_keyframe = false;
        }

        if is_keyframe {
            self.segment_has_keyframe = true;
        }
        self.segment_last_dts_ms = dts_ms;

        if let Some(prev) = self.prev_dts_ms {
            if dts_ms > prev {
                self.last_frame_interval_ms = Some(dts_ms - prev);
            }
        }
        self.prev_dts_ms = Some(dts_ms);

        // Part cut decision
        let should_cut_part = self.ll_state.should_cut_part(dts_ms);
        if should_cut_part {
            if let Some(part) = self.finalize_current_part(Some(dts_ms)) {
                outputs.push(TrackMuxerOutput::PartReady(part));
            }
        }

        // Track id is always 1 within a single-track fMP4
        let sample = Fmp4Sample {
            track_id: 1,
            pts_ms,
            dts_ms,
            is_keyframe,
            data,
        };

        self.ll_state.note_sample(dts_ms, is_keyframe);
        self.pending_part_samples.push(sample);

        outputs
    }

    /// Flush remaining data (conclude stream).
    pub fn flush(&mut self) -> Vec<TrackMuxerOutput> {
        let mut outputs = Vec::new();
        if let Some(part) = self.finalize_current_part(None) {
            outputs.push(TrackMuxerOutput::PartReady(part));
        }
        if self.segment_start_dts_ms.is_some() {
            if let Some(seg_out) = self.finalize_segment(None) {
                outputs.push(seg_out);
            }
        }
        self.concluded = true;
        outputs
    }

    /// Get a part by global sequence number.
    pub fn get_part(&self, seq: u64) -> Option<Bytes> {
        self.ll_state.get_part(seq).map(|p| p.data.clone())
    }

    /// Get a segment by name.
    pub fn get_segment(&self, name: &str) -> Option<Bytes> {
        self.ring.get(name).map(|s| s.data.clone())
    }

    /// Current segment sequence (MSN).
    pub fn current_msn(&self) -> u64 {
        self.segment_seq
    }

    /// Next part sequence.
    pub fn next_part_seq(&self) -> u64 {
        self.ll_state.next_part_seq()
    }

    /// Check if a blocking request is satisfied.
    pub fn is_blocking_satisfied(&self, target_msn: u64, target_part: Option<u64>) -> bool {
        if self.concluded {
            return true;
        }
        let current_msn = self.ll_state.parent_segment_seq();
        match target_part {
            Some(tp) => {
                if current_msn > target_msn {
                    return true;
                }
                if current_msn == target_msn {
                    return self.ll_state.current_parts().len() as u64 > tp;
                }
                false
            }
            None => current_msn > target_msn,
        }
    }

    /// Part target in seconds.
    pub fn part_target_secs(&self) -> f64 {
        self.part_target_ms as f64 / 1000.0
    }

    /// Set wallclock offset for PROGRAM-DATE-TIME calculation.
    pub fn set_wallclock_offset(&mut self, offset_ms: i64) {
        self.wallclock_offset_ms = Some(offset_ms);
    }

    fn finalize_current_part(&mut self, end_dts_ms: Option<u64>) -> Option<HlsPart> {
        if self.pending_part_samples.is_empty() {
            return None;
        }
        let samples = std::mem::take(&mut self.pending_part_samples);
        let first_dts_ms = samples.first().map(|s| s.dts_ms).unwrap_or(0);
        let duration_secs = match end_dts_ms {
            Some(end) => end.saturating_sub(first_dts_ms) as f64 / 1000.0,
            None => {
                let last_dts_ms = samples.last().map(|s| s.dts_ms).unwrap_or(first_dts_ms);
                let d = last_dts_ms.saturating_sub(first_dts_ms) as f64 / 1000.0;
                if d <= 0.0 {
                    self.part_target_ms as f64 / 1000.0
                } else {
                    d
                }
            }
        };
        let data = self.fmp4_muxer.write_part(&samples);
        self.pending_segment_part_data.push(data.clone());
        let part = self.ll_state.finalize_part(data, duration_secs);
        Some(part)
    }

    fn finalize_segment(&mut self, next_dts_ms: Option<u64>) -> Option<TrackMuxerOutput> {
        let start = self.segment_start_dts_ms.take()?;

        // Segment = concatenation of all its parts
        if self.pending_segment_part_data.is_empty() {
            return None;
        }
        let parts = std::mem::take(&mut self.pending_segment_part_data);
        let total_len: usize = parts.iter().map(|p| p.len()).sum();
        let mut combined = bytes::BytesMut::with_capacity(total_len);
        for p in &parts {
            combined.extend_from_slice(p);
        }
        let data = combined.freeze();

        let duration_ms = match next_dts_ms {
            Some(end) if end > start => end - start,
            _ => {
                let span = self.segment_last_dts_ms.saturating_sub(start);
                span + self.last_frame_interval_ms.unwrap_or(33)
            }
        };
        let duration_secs = duration_ms as f64 / 1000.0;

        let name = format!("{}_seg_{}", self.lane.prefix(), self.segment_seq);
        let seg_seq = self.segment_seq;
        self.segment_seq += 1;

        let pdt_ms = self.wallclock_offset_ms.map(|offset| start as i64 + offset);
        self.ring.push_with_pdt(
            name.clone(),
            duration_secs,
            data,
            self.segment_has_keyframe,
            pdt_ms,
        );
        self.ll_state.on_segment_boundary(seg_seq + 1);
        self.pending_part_samples.clear();
        self.segment_has_keyframe = false;

        Some(TrackMuxerOutput::SegmentReady {
            name,
            duration_secs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_video_track_desc() -> Fmp4TrackDesc {
        Fmp4TrackDesc {
            track_id: 1,
            codec: CodecId::H264,
            media_kind: MediaKind::Video,
            timescale: 90000,
            extradata: Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1e, 0xff, 0xe1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1e, 0x01, 0x00,
                0x03, 0x68, 0xce, 0x38,
            ]),
            width: 1920,
            height: 1080,
            sample_rate: 0,
            channels: 0,
        }
    }

    fn make_audio_track_desc() -> Fmp4TrackDesc {
        Fmp4TrackDesc {
            track_id: 1,
            codec: CodecId::AAC,
            media_kind: MediaKind::Audio,
            timescale: 44100,
            extradata: Bytes::from_static(&[0x12, 0x10]),
            width: 0,
            height: 0,
            sample_rate: 44100,
            channels: 2,
        }
    }

    #[test]
    fn demuxed_video_init_has_one_video_track() {
        let muxer = TrackMuxer::new(
            TrackLane::Video,
            TrackId(1),
            make_video_track_desc(),
            200,
            3,
            5,
        );
        let init = &muxer.init_segment;
        // init segment should contain moov with a single trak
        assert!(init.windows(4).any(|w| w == b"moov"));
        let trak_count = init.windows(4).filter(|w| *w == b"trak").count();
        assert_eq!(trak_count, 1);
    }

    #[test]
    fn demuxed_audio_init_has_one_audio_track() {
        let muxer = TrackMuxer::new(
            TrackLane::Audio,
            TrackId(2),
            make_audio_track_desc(),
            200,
            3,
            5,
        );
        let init = &muxer.init_segment;
        assert!(init.windows(4).any(|w| w == b"moov"));
        let trak_count = init.windows(4).filter(|w| *w == b"trak").count();
        assert_eq!(trak_count, 1);
    }

    #[test]
    fn demuxed_video_part_has_single_traf() {
        let mut muxer = TrackMuxer::new(
            TrackLane::Video,
            TrackId(1),
            make_video_track_desc(),
            200,
            3,
            5,
        );
        // Push frames to produce a part (200ms target, 33ms per frame)
        // Need 7+ frames: frame at dts=231 triggers cut of frames 0-6
        for i in 0..8 {
            let dts_ms = i * 33;
            muxer.push_sample(
                dts_ms,
                dts_ms,
                i == 0,
                Bytes::from(vec![0u8; 100]),
                4000,
                10000,
            );
        }
        let part_data = muxer.get_part(0).expect("part 0 should exist");
        assert!(part_data.windows(4).any(|w| w == b"moof"));
        let traf_count = part_data.windows(4).filter(|w| *w == b"traf").count();
        assert_eq!(traf_count, 1);
    }

    #[test]
    fn demuxed_audio_part_has_single_traf() {
        let mut muxer = TrackMuxer::new(
            TrackLane::Audio,
            TrackId(2),
            make_audio_track_desc(),
            209, // ~9 AAC frames at 44100
            3,
            5,
        );
        // AAC frame duration at 44100 = 1024/44100*1000 ≈ 23.2ms
        // Need 10+ frames to exceed 209ms target
        for i in 0..11 {
            let dts_ms = (i as f64 * 23.2) as u64;
            muxer.push_sample(
                dts_ms,
                dts_ms,
                true,
                Bytes::from(vec![0u8; 50]),
                4000,
                10000,
            );
        }
        let part_data = muxer.get_part(0).expect("audio part 0 should exist");
        let traf_count = part_data.windows(4).filter(|w| *w == b"traf").count();
        assert_eq!(traf_count, 1);
    }

    #[test]
    fn flush_produces_segment() {
        let mut muxer = TrackMuxer::new(
            TrackLane::Video,
            TrackId(1),
            make_video_track_desc(),
            200,
            3,
            5,
        );
        for i in 0..7 {
            let dts_ms = i * 33;
            muxer.push_sample(
                dts_ms,
                dts_ms,
                i == 0,
                Bytes::from(vec![0u8; 100]),
                4000,
                10000,
            );
        }
        let outputs = muxer.flush();
        assert!(outputs
            .iter()
            .any(|o| matches!(o, TrackMuxerOutput::SegmentReady { .. })));
        assert!(muxer.concluded);
    }
}
