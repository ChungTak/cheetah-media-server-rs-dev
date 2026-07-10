//! HLS record container writer (fMP4 segments + VOD playlist).
//!
//! Wraps `cheetah_codec::Fmp4Muxer` to produce segments and emits a VOD
//! playlist on `finalize`. The driver is responsible for actually writing
//! init/segment files and the playlist.

use crate::prelude::*;

use bytes::Bytes;

use crate::fmp4_mux::{Fmp4MuxEvent, Fmp4MuxSample, Fmp4Muxer, Fmp4MuxerConfig};
use crate::frame::{AVFrame, FrameFlags};
use crate::track::TrackInfo;

use super::{RecordContainerWriter, RecordError, RecordFormat, RecordWriteEvent};

/// Configuration for HLS record output.
#[derive(Debug, Clone)]
pub struct HlsFileWriterConfig {
    /// `segment_duration_ms` field of type `u64`.
    /// `segment_duration_ms` 字段，类型为 `u64`.
    pub segment_duration_ms: u64,
}

impl Default for HlsFileWriterConfig {
    fn default() -> Self {
        Self {
            segment_duration_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SegmentMeta {
    seq: u32,
    duration_ms: u64,
    path_hint: String,
}

/// Stateful HLS record writer.
pub struct HlsFileWriter {
    /// `config` field of type `HlsFileWriterConfig`.
    /// `config` 字段，类型为 `HlsFileWriterConfig`.
    config: HlsFileWriterConfig,
    /// `muxer` field.
    /// `muxer` 字段.
    muxer: Option<Fmp4Muxer>,
    /// `init_emitted` field of type `bool`.
    /// `init_emitted` 字段，类型为 `bool`.
    init_emitted: bool,
    /// `pending_samples` field.
    /// `pending_samples` 字段.
    pending_samples: Vec<Fmp4MuxSample>,
    /// `segment_start_dts_us` field.
    /// `segment_start_dts_us` 字段.
    segment_start_dts_us: Option<i64>,
    /// `next_seq` field of type `u32`.
    /// `next_seq` 字段，类型为 `u32`.
    next_seq: u32,
    /// `finalized` field of type `bool`.
    /// `finalized` 字段，类型为 `bool`.
    finalized: bool,
    /// `segments` field.
    /// `segments` 字段.
    segments: Vec<SegmentMeta>,
}

impl HlsFileWriter {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(config: HlsFileWriterConfig) -> Self {
        Self {
            config,
            muxer: None,
            init_emitted: false,
            pending_samples: Vec::new(),
            segment_start_dts_us: None,
            next_seq: 0,
            finalized: false,
            segments: Vec::new(),
        }
    }

    fn flush_segment(&mut self, last_dts_us: i64) -> Vec<RecordWriteEvent> {
        let mut out = Vec::new();
        let Some(muxer) = self.muxer.as_mut() else {
            return out;
        };
        if self.pending_samples.is_empty() {
            return out;
        }
        let events = muxer.write_segment(&self.pending_samples);
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let path = format!("seg-{seq:05}.m4s");
        for e in events {
            match e {
                Fmp4MuxEvent::MediaSegment { data, keyframe } => {
                    out.push(RecordWriteEvent::Segment {
                        path_hint: path.clone(),
                        bytes: data,
                        keyframe,
                    });
                }
                Fmp4MuxEvent::InitSegment(_) | Fmp4MuxEvent::Diagnostic(_) => {}
            }
        }
        let duration_ms = last_dts_us
            .saturating_sub(self.segment_start_dts_us.unwrap_or(0))
            .max(0)
            .saturating_div(1000) as u64;
        self.segments.push(SegmentMeta {
            seq,
            duration_ms,
            path_hint: path,
        });
        self.pending_samples.clear();
        self.segment_start_dts_us = None;
        out
    }
}

impl RecordContainerWriter for HlsFileWriter {
    fn update_tracks(&mut self, tracks: &[TrackInfo]) -> Result<(), RecordError> {
        if tracks.is_empty() {
            return Err(RecordError::InvalidTracks("no tracks"));
        }
        self.muxer = Some(Fmp4Muxer::new(Fmp4MuxerConfig::default(), tracks));
        Ok(())
    }

    fn push_frame(&mut self, frame: &AVFrame) -> Result<Vec<RecordWriteEvent>, RecordError> {
        if self.finalized {
            return Err(RecordError::Finalized);
        }
        let muxer = self.muxer.as_mut().ok_or(RecordError::NotInitialized)?;
        let mut out = Vec::new();

        if !self.init_emitted {
            for e in muxer.init_segment() {
                if let Fmp4MuxEvent::InitSegment(data) = e {
                    out.push(RecordWriteEvent::InitSegment {
                        path_hint: "init.mp4".to_string(),
                        bytes: data,
                    });
                }
            }
            self.init_emitted = true;
        }

        let seg_start = self.segment_start_dts_us.get_or_insert(frame.dts_us);
        let elapsed_ms = ((frame.dts_us - *seg_start).max(0)) as u64 / 1000;
        let is_key = frame.flags.contains(FrameFlags::KEY);
        if elapsed_ms >= self.config.segment_duration_ms && is_key {
            out.extend(self.flush_segment(frame.dts_us));
        }

        self.pending_samples.push(Fmp4MuxSample {
            track_id: frame.track_id.0,
            dts_us: frame.dts_us,
            pts_us: frame.pts_us,
            is_keyframe: is_key,
            data: frame.payload.clone(),
        });

        Ok(out)
    }

    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError> {
        if self.finalized {
            return Ok(Vec::new());
        }
        let last_dts = self
            .pending_samples
            .last()
            .map(|s| s.dts_us)
            .unwrap_or(self.segment_start_dts_us.unwrap_or(0));
        let mut out = self.flush_segment(last_dts);

        // VOD playlist
        let mut playlist = String::new();
        playlist.push_str("#EXTM3U\n");
        playlist.push_str("#EXT-X-VERSION:7\n");
        playlist.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
        let max = self
            .segments
            .iter()
            .map(|s| s.duration_ms.div_ceil(1000))
            .max()
            .unwrap_or(1);
        playlist.push_str(&format!("#EXT-X-TARGETDURATION:{max}\n"));
        playlist.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n");
        for seg in &self.segments {
            let secs = seg.duration_ms as f64 / 1000.0;
            playlist.push_str(&format!("#EXTINF:{:.3},\n{}\n", secs, seg.path_hint));
        }
        playlist.push_str("#EXT-X-ENDLIST\n");
        out.push(RecordWriteEvent::Playlist {
            path_hint: "index.m3u8".to_string(),
            body: Bytes::from(playlist),
        });
        self.finalized = true;
        Ok(out)
    }

    fn format(&self) -> RecordFormat {
        RecordFormat::Hls
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameFormat;
    use crate::time::Timebase;
    use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo};

    fn h264_track() -> TrackInfo {
        let mut t = TrackInfo::new(TrackId(1), MediaKind::Video, CodecId::H264, 90_000);
        t.width = Some(640);
        t.height = Some(360);
        t.extradata = CodecExtradata::H264 {
            sps: vec![],
            pps: vec![],
            avcc: Some(Bytes::from_static(&[
                0x01, 0x42, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x04, 0x67, 0x42, 0x00, 0x1E, 0x01, 0x00,
                0x03, 0x68, 0xCE, 0x38,
            ])),
        };
        t
    }

    #[test]
    fn writes_vod_playlist_on_finalize() {
        let mut w = HlsFileWriter::new(HlsFileWriterConfig::default());
        w.update_tracks(&[h264_track()]).unwrap();
        let tb = Timebase::new(1, 90_000);
        for i in 0..3 {
            let mut f = AVFrame::new(
                TrackId(1),
                MediaKind::Video,
                CodecId::H264,
                FrameFormat::CanonicalH26x,
                i * 90_000 / 30,
                i * 90_000 / 30,
                tb,
                Bytes::from_static(b"AU"),
            );
            if i == 0 {
                f.flags.insert(FrameFlags::KEY);
            }
            w.push_frame(&f).unwrap();
        }
        let evs = w.finalize().unwrap();
        let has_playlist = evs
            .iter()
            .any(|e| matches!(e, RecordWriteEvent::Playlist { .. }));
        assert!(has_playlist);
    }
}
