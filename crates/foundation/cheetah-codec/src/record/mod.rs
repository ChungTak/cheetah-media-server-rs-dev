//! Unified record container writer abstraction shared by `cheetah-record-module`.
//!
//! This module defines the runtime-neutral record writer trait and event
//! types. Concrete writers live in `record/flv.rs`, `record/mp4.rs`,
//! `record/hls.rs`, `record/ps.rs`. The runtime is responsible for actual
//! disk I/O.

/// Module for `flv`.
/// `flv` 相关模块。
pub mod flv;
/// Module for `hls`.
/// `hls` 相关模块。
pub mod hls;
/// Module for `mp4`.
/// `mp4` 相关模块。
pub mod mp4;
/// Module for `ps`.
/// `ps` 相关模块。
pub mod ps;

use crate::prelude::*;
use bytes::Bytes;

use crate::frame::AVFrame;
use crate::track::TrackInfo;

/// Supported record file containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordFormat {
    Flv,
    Hls,
    Mp4,
    Ps,
}

impl RecordFormat {
    /// `extension` function of `RecordFormat`.
    /// `RecordFormat` 的 `extension` 函数。
    pub fn extension(self) -> &'static str {
        match self {
            RecordFormat::Flv => "flv",
            RecordFormat::Hls => "m3u8",
            RecordFormat::Mp4 => "mp4",
            RecordFormat::Ps => "ps",
        }
    }

    /// Parses the input into a structured value, returning an error if malformed.
    /// 将输入解析为结构化值，格式错误时返回错误。
    pub fn parse(input: &str) -> Option<Self> {
        let lower = input.to_ascii_lowercase();
        match lower.as_str() {
            "flv" => Some(RecordFormat::Flv),
            "hls" => Some(RecordFormat::Hls),
            "mp4" => Some(RecordFormat::Mp4),
            "ps" => Some(RecordFormat::Ps),
            _ => None,
        }
    }
}

/// Output produced by a record writer. The runtime layer transports these to
/// the disk I/O subsystem.
#[derive(Debug, Clone)]
pub enum RecordWriteEvent {
    /// Append raw bytes to the active file.
    Bytes(Bytes),
    /// Emit a complete media segment (e.g. an HLS .m4s) to the named slot.
    Segment {
        path_hint: String,
        bytes: Bytes,
        keyframe: bool,
    },
    /// Emit an HLS init segment.
    InitSegment { path_hint: String, bytes: Bytes },
    /// Emit an HLS playlist (live or VOD).
    Playlist { path_hint: String, body: Bytes },
    /// Bounded structured diagnostic.
    Diagnostic(RecordDiagnostic),
}

/// Diagnostic emitted by a record writer.
#[derive(Debug, Clone)]
pub enum RecordDiagnostic {
    UnsupportedCodec {
        codec: crate::track::CodecId,
        track_id: u32,
    },
    UnsupportedTrack {
        track_id: u32,
        reason: &'static str,
    },
    MalformedFrame {
        track_id: u32,
        reason: &'static str,
    },
    BackpressureDropped {
        track_id: u32,
        count: u32,
    },
    /// The writer asks the runtime layer to discard the file because it is
    /// below the configured "valid recording" threshold (see ZLM
    /// `MP4Recorder::asyncClose()` 1024-byte rule). The runtime should
    /// remove the partially written `.part` file rather than rename it.
    DropTinyFile {
        size_bytes: u64,
        threshold_bytes: u64,
    },
}

/// Errors produced by record writers.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RecordError {
    #[error("track configuration invalid: {0}")]
    InvalidTracks(&'static str),
    #[error("frame rejected: {0}")]
    InvalidFrame(&'static str),
    #[error("writer is not initialized")]
    NotInitialized,
    #[error("writer is finalized")]
    Finalized,
    #[error("internal error: {0}")]
    Internal(&'static str),
}

/// Container-agnostic record writer trait.
pub trait RecordContainerWriter: Send {
    /// Update the active track set. Called once after opening; may be called
    /// again if upstream re-emits track info (re-sync).
    fn update_tracks(&mut self, tracks: &[TrackInfo]) -> Result<(), RecordError>;

    /// Push an `AVFrame` into the writer. Returns 0 or more events.
    fn push_frame(&mut self, frame: &AVFrame) -> Result<Vec<RecordWriteEvent>, RecordError>;

    /// Finalize the writer; flushes any pending data and emits trailing
    /// boxes/playlists.
    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError>;

    /// Container format the writer produces.
    fn format(&self) -> RecordFormat;
}

/// Boxed writer alias the record module uses to dispatch by format.
pub type DynRecordWriter = Box<dyn RecordContainerWriter>;

/// Build a default writer for the given format. This is a convenience for
/// testing; production callers may pass a custom config to the per-format
/// writer constructors directly.
pub fn make_default_writer(format: RecordFormat) -> DynRecordWriter {
    match format {
        RecordFormat::Flv => Box::new(flv::FlvFileWriter::new(flv::FlvFileWriterConfig::default())),
        RecordFormat::Hls => Box::new(hls::HlsFileWriter::new(hls::HlsFileWriterConfig::default())),
        RecordFormat::Mp4 => Box::new(mp4::Mp4FileWriter::new(mp4::Mp4FileWriterConfig::default())),
        RecordFormat::Ps => Box::new(ps::PsFileWriter::new(ps::PsFileWriterConfig::default())),
    }
}

// Re-export so callers don't have to dig in.
pub use bytes;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_format_extensions() {
        assert_eq!(RecordFormat::Flv.extension(), "flv");
        assert_eq!(RecordFormat::Mp4.extension(), "mp4");
        assert_eq!(RecordFormat::Hls.extension(), "m3u8");
        assert_eq!(RecordFormat::Ps.extension(), "ps");
    }

    #[test]
    fn record_format_parse() {
        assert_eq!(RecordFormat::parse("flv"), Some(RecordFormat::Flv));
        assert_eq!(RecordFormat::parse("Mp4"), Some(RecordFormat::Mp4));
        assert_eq!(RecordFormat::parse("HLS"), Some(RecordFormat::Hls));
        assert_eq!(RecordFormat::parse("ps"), Some(RecordFormat::Ps));
        assert_eq!(RecordFormat::parse("ts"), None);
    }
}
