//! PS (program stream) record writer.
//!
//! Wraps `cheetah_codec::ps::PsMuxer` to produce PS bytes for a recording
//! file. Used primarily by GB28181 record paths.
//!
//! PS（节目流）录制写入器。
//!
//! 封装 `cheetah_codec::ps::PsMuxer` 为录制文件生成 PS 字节。
//! 主要用于 GB28181 录制路径。

use crate::prelude::*;

use crate::frame::AVFrame;
use crate::ps::PsMuxer;
use crate::track::TrackInfo;

use super::{RecordContainerWriter, RecordDiagnostic, RecordError, RecordFormat, RecordWriteEvent};

/// PS file writer configuration.
///
/// PS 文件写入器配置。
#[derive(Debug, Clone, Default)]
pub struct PsFileWriterConfig {}

/// Stateful PS file writer.
///
/// 有状态 PS 文件写入器。
pub struct PsFileWriter {
    inner: PsMuxer,
    initialized: bool,
    finalized: bool,
}

impl PsFileWriter {
    /// Create a new PS writer with the given configuration.
    ///
    /// 使用给定配置创建新的 PS 写入器。
    pub fn new(_config: PsFileWriterConfig) -> Self {
        Self {
            inner: PsMuxer::new(),
            initialized: false,
            finalized: false,
        }
    }
}

impl RecordContainerWriter for PsFileWriter {
    /// Rebuild the internal `PsMuxer` with the latest track set.
    ///
    /// 使用最新的轨道集重建内部 `PsMuxer`。
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

    /// Mux a frame through `PsMuxer` and emit the produced PS bytes.
    ///
    /// 通过 `PsMuxer` 复用帧并输出生成的 PS 字节。
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

    /// Mark the writer as finalized. PS muxer requires no trailing data.
    ///
    /// 标记写入器已完成。PS 复用器不需要尾部数据。
    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError> {
        self.finalized = true;
        Ok(Vec::new())
    }

    /// 返回 `RecordFormat::Ps`。
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
