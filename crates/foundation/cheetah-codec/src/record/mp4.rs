//! Classic MP4 record container writer.
//!
//! Buffers samples until `finalize` is called, then emits a single MP4 file
//! via `cheetah_codec::mp4::Mp4Writer`.

use crate::prelude::*;

use crate::frame::{AVFrame, FrameFlags};
use crate::frame_view::h26x_length_prefixed_from_payload;
use crate::mp4::{Mp4WriteEvent, Mp4Writer, Mp4WriterConfig};
use crate::track::{CodecId, TrackInfo};

use super::{RecordContainerWriter, RecordDiagnostic, RecordError, RecordFormat, RecordWriteEvent};

/// Writer configuration.
#[derive(Debug, Clone, Default)]
pub struct Mp4FileWriterConfig {
    /// Reserved for the future faststart layout. Currently must be false;
    /// `Mp4Writer::finalize` rejects `true` until the two-pass writer
    /// lands.
    pub faststart: bool,
    /// ZLM-compat: if the finalized buffer is shorter than this many bytes,
    /// `finalize` returns a `DropFile` diagnostic instead of a `Bytes` event
    /// so the disk-writer layer drops it. Mirrors
    /// `MP4Recorder::asyncClose()`'s 1024-byte heuristic.
    pub drop_below_bytes: u64,
}

/// Stateful MP4 file record writer.
pub struct Mp4FileWriter {
    /// `config` field of type `Mp4FileWriterConfig`.
    /// `config` 字段，类型为 `Mp4FileWriterConfig`.
    config: Mp4FileWriterConfig,
    /// `inner` field.
    /// `inner` 字段.
    inner: Option<Mp4Writer>,
    /// `tracks` field.
    /// `tracks` 字段.
    tracks: Vec<TrackInfo>,
    /// `finalized` field of type `bool`.
    /// `finalized` 字段，类型为 `bool`.
    finalized: bool,
    /// `drop_count` field of type `u32`.
    /// `drop_count` 字段，类型为 `u32`.
    drop_count: u32,
}

impl Mp4FileWriter {
    /// Creates a new instance.
    /// 创建 新的 实例.
    pub fn new(config: Mp4FileWriterConfig) -> Self {
        Self {
            config,
            inner: None,
            tracks: Vec::new(),
            finalized: false,
            drop_count: 0,
        }
    }

    /// Number of frames the writer has rejected since open. Useful for
    /// surfacing health metrics through the record module.
    pub fn drop_count(&self) -> u32 {
        self.drop_count
    }
}

impl RecordContainerWriter for Mp4FileWriter {
    fn update_tracks(&mut self, tracks: &[TrackInfo]) -> Result<(), RecordError> {
        if tracks.is_empty() {
            return Err(RecordError::InvalidTracks("no tracks"));
        }
        let writer_config = Mp4WriterConfig {
            faststart: self.config.faststart,
            ..Default::default()
        };
        let writer = Mp4Writer::new(writer_config, tracks)
            .map_err(|_| RecordError::InvalidTracks("track unsupported"))?;
        self.inner = Some(writer);
        self.tracks = tracks.to_vec();
        Ok(())
    }

    fn push_frame(&mut self, frame: &AVFrame) -> Result<Vec<RecordWriteEvent>, RecordError> {
        if self.finalized {
            return Err(RecordError::Finalized);
        }
        let writer = self.inner.as_mut().ok_or(RecordError::NotInitialized)?;
        let is_sync = frame.flags.contains(FrameFlags::KEY);
        // MP4 carries H.26x access units in length-prefixed (AVCC/HVCC) form,
        // not Annex-B. The engine's canonical H26x payload may be Annex-B
        // (start-code prefixed); convert it before muxing so ffmpeg can
        // decode the resulting file. `h26x_length_prefixed_from_payload`
        // is a no-op when the payload already arrives length-prefixed.
        let payload_buf;
        let sample_data: &[u8] = match frame.codec {
            CodecId::H264 | CodecId::H265 | CodecId::H266 => {
                payload_buf = h26x_length_prefixed_from_payload(frame.payload.clone());
                payload_buf.as_ref()
            }
            _ => frame.payload.as_ref(),
        };
        let push = writer.push_sample(
            frame.track_id.0,
            frame.dts_us,
            frame.pts_us,
            is_sync,
            sample_data,
        );
        if push.is_err() {
            self.drop_count = self.drop_count.saturating_add(1);
            return Ok(vec![RecordWriteEvent::Diagnostic(
                RecordDiagnostic::MalformedFrame {
                    track_id: frame.track_id.0,
                    reason: "writer rejected sample",
                },
            )]);
        }
        Ok(Vec::new())
    }

    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError> {
        if self.finalized {
            return Ok(Vec::new());
        }
        self.finalized = true;
        let inner = self.inner.take().ok_or(RecordError::NotInitialized)?;
        let Mp4WriteEvent::File(buf) = inner
            .finalize()
            .map_err(|_| RecordError::Internal("mp4 finalize failed"))?;
        if self.config.drop_below_bytes > 0 && (buf.len() as u64) < self.config.drop_below_bytes {
            return Ok(vec![RecordWriteEvent::Diagnostic(
                RecordDiagnostic::DropTinyFile {
                    size_bytes: buf.len() as u64,
                    threshold_bytes: self.config.drop_below_bytes,
                },
            )]);
        }
        Ok(vec![RecordWriteEvent::Bytes(buf)])
    }

    fn format(&self) -> RecordFormat {
        RecordFormat::Mp4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameFormat;
    use crate::time::Timebase;
    use crate::track::{CodecExtradata, CodecId, MediaKind, TrackId, TrackInfo};
    use bytes::Bytes;

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
    fn produces_mp4_file_after_finalize() {
        let mut w = Mp4FileWriter::new(Mp4FileWriterConfig::default());
        w.update_tracks(&[h264_track()]).unwrap();
        let tb = Timebase::new(1, 90_000);
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            tb,
            Bytes::from_static(b"AU"),
        );
        frame.flags.insert(FrameFlags::KEY);
        let pre = w.push_frame(&frame).unwrap();
        assert!(pre.is_empty());
        let final_evs = w.finalize().unwrap();
        assert!(matches!(final_evs[0], RecordWriteEvent::Bytes(_)));
    }

    #[test]
    fn drops_below_threshold_yields_diagnostic() {
        let cfg = Mp4FileWriterConfig {
            drop_below_bytes: 1024 * 1024,
            ..Default::default()
        };
        let mut w = Mp4FileWriter::new(cfg);
        w.update_tracks(&[h264_track()]).unwrap();
        let tb = Timebase::new(1, 90_000);
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            tb,
            Bytes::from_static(b"AU"),
        );
        frame.flags.insert(FrameFlags::KEY);
        w.push_frame(&frame).unwrap();
        let final_evs = w.finalize().unwrap();
        assert!(matches!(
            final_evs[0],
            RecordWriteEvent::Diagnostic(RecordDiagnostic::DropTinyFile { .. })
        ));
    }

    #[test]
    fn h264_annexb_payload_is_converted_to_length_prefixed() {
        // The engine canonical H264 payload is Annex-B (start-code prefixed).
        // The MP4 sample format requires AVCC length-prefixed NAL units; the
        // writer must convert before muxing or ffmpeg will report
        // "Invalid NAL unit size" / "missing picture in access unit".
        let mut w = Mp4FileWriter::new(Mp4FileWriterConfig::default());
        w.update_tracks(&[h264_track()]).unwrap();
        let tb = Timebase::new(1, 90_000);
        // Two NAL units back-to-back in Annex-B form.
        let annexb: &[u8] = &[
            0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x00, 0x00, 0x00, 0x01, 0x41, 0x9a,
        ];
        let mut frame = AVFrame::new(
            TrackId(1),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            tb,
            Bytes::copy_from_slice(annexb),
        );
        frame.flags.insert(FrameFlags::KEY);
        w.push_frame(&frame).unwrap();
        let final_evs = w.finalize().unwrap();
        let RecordWriteEvent::Bytes(buf) = &final_evs[0] else {
            panic!("expected mp4 bytes, got {:?}", &final_evs[0]);
        };
        // Each NAL is rewritten with a 4-byte length prefix; verify the
        // file does NOT contain raw Annex-B start codes inside the mdat
        // payload window for the sample. We search for 00 00 00 01 followed
        // by 0x65 (the IDR NAL header) and assert it is absent.
        let needle: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84];
        assert!(
            !buf.windows(needle.len()).any(|w| w == needle),
            "annex-b start code leaked into mp4 sample data"
        );
        // The length-prefixed form embeds the NAL header (0x65) immediately
        // after a 4-byte big-endian length. The first NAL is 3 bytes
        // (`65 88 84`), so the prefix is `00 00 00 03`.
        let lp_needle: &[u8] = &[0x00, 0x00, 0x00, 0x03, 0x65, 0x88, 0x84];
        assert!(
            buf.windows(lp_needle.len()).any(|w| w == lp_needle),
            "length-prefixed NAL not found"
        );
    }
}
