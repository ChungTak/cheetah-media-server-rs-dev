//! PS (program stream) record writer.
//!
//! Wraps `cheetah_codec::ps::PsMuxer` to produce PS bytes for a recording
//! file. Used primarily by GB28181 record paths.

use crate::prelude::*;

use crate::frame::AVFrame;
use crate::ps::PsMuxer;
use crate::track::TrackInfo;

use super::{RecordContainerWriter, RecordDiagnostic, RecordError, RecordFormat, RecordWriteEvent};

/// Configuration for `Ps File Writer`.
/// `Ps File Writer` 的配置。
#[derive(Debug, Clone, Default)]
pub struct PsFileWriterConfig {}

/// `PsFileWriter` data structure.
/// `PsFileWriter` 数据结构。
pub struct PsFileWriter {
    inner: PsMuxer,
    initialized: bool,
    finalized: bool,
}

impl PsFileWriter {
    /// Creates a new `PsFileWriter` instance.
    /// 创建新的 `PsFileWriter` 实例。
    pub fn new(_config: PsFileWriterConfig) -> Self {
        Self {
            inner: PsMuxer::new(),
            initialized: false,
            finalized: false,
        }
    }
}

impl RecordContainerWriter for PsFileWriter {
    fn update_tracks(&mut self, tracks: &[TrackInfo]) -> Result<(), RecordError> {
        if tracks.is_empty() {
            return Err(RecordError::InvalidTracks("no tracks"));
        }
        // Re-sync semantics: each call replaces the previous track set so a
        // mid-stream re-init does not pile registrations into the muxer.
        self.inner = PsMuxer::new();
        for t in tracks {
            self.inner.add_track(t.clone());
        }
        self.initialized = true;
        Ok(())
    }

    fn push_frame(&mut self, frame: &AVFrame) -> Result<Vec<RecordWriteEvent>, RecordError> {
        if self.finalized {
            return Err(RecordError::Finalized);
        }
        if !self.initialized {
            return Err(RecordError::NotInitialized);
        }
        match self.inner.mux(frame) {
            Some(bytes) => Ok(vec![RecordWriteEvent::Bytes(bytes)]),
            None => Ok(vec![RecordWriteEvent::Diagnostic(
                RecordDiagnostic::UnsupportedTrack {
                    track_id: frame.track_id.0,
                    reason: "track not registered with ps muxer",
                },
            )]),
        }
    }

    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError> {
        self.finalized = true;
        Ok(Vec::new())
    }

    fn format(&self) -> RecordFormat {
        RecordFormat::Ps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{AVFrame, FrameFlags, FrameFormat};
    use crate::time::Timebase;
    use crate::track::{CodecId, MediaKind, TrackId, TrackInfo};
    use bytes::Bytes;

    #[test]
    fn ps_writer_emits_bytes_for_h264_keyframe() {
        let track = TrackInfo::new(TrackId(0xE0), MediaKind::Video, CodecId::H264, 90_000);
        let mut w = PsFileWriter::new(PsFileWriterConfig::default());
        w.update_tracks(&[track]).unwrap();
        let tb = Timebase::new(1, 90_000);
        let mut f = AVFrame::new(
            TrackId(0xE0),
            MediaKind::Video,
            CodecId::H264,
            FrameFormat::CanonicalH26x,
            0,
            0,
            tb,
            Bytes::from_static(b"AU"),
        );
        f.flags.insert(FrameFlags::KEY);
        let evs = w.push_frame(&f).unwrap();
        assert!(!evs.is_empty());
    }
}
