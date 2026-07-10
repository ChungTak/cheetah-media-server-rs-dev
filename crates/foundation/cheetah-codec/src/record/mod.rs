//! Unified record container writer abstraction shared by `cheetah-record-module`.
//!
//! This module defines the runtime-neutral record writer trait and event
//! types. Concrete writers live in `record/flv.rs`, `record/mp4.rs`,
//! `record/hls.rs`, `record/ps.rs`. The runtime is responsible for actual
//! disk I/O.
//!
//! `cheetah-record-module` 共享的统一录制容器写入器抽象。
//!
//! 本模块定义了运行时无关的录制写入器 trait 与事件类型。具体写入器
//! 位于 `record/flv.rs`、`record/mp4.rs`、`record/hls.rs`、`record/ps.rs`。
//! 运行时负责实际磁盘 I/O。

pub mod flv;
pub mod hls;
pub mod mp4;
pub mod ps;

use crate::prelude::*;
use bytes::Bytes;

use crate::frame::AVFrame;
use crate::track::TrackInfo;

/// Supported record file containers.
///
/// Each variant maps to a file extension and can be parsed from a string so
/// the record module can dispatch to the right concrete writer.
///
/// 支持的录制文件容器。
///
/// 每个变体映射到一个文件扩展名，并可通过字符串解析，
/// 使录制模块能分派到正确的具体写入器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordFormat {
    Flv,
    Hls,
    Mp4,
    Ps,
}

impl RecordFormat {
    /// File extension for this container.
    ///
    /// 该容器对应的文件扩展名。
    pub fn extension(self) -> &'static str {
        match self {
            RecordFormat::Flv => "flv",
            RecordFormat::Hls => "m3u8",
            RecordFormat::Mp4 => "mp4",
            RecordFormat::Ps => "ps",
        }
    }

    /// Parse a record format from a case-insensitive string.
    ///
    /// 从不区分大小写的字符串解析录制格式。
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
///
/// The variant chosen communicates whether the runtime should append bytes,
/// write a named segment, emit an HLS init/pl, or surface a diagnostic.
///
/// 录制写入器产生的输出。运行时层将其传输到磁盘 I/O 子系统。
///
/// 所选变体告知运行时是追加字节、写入命名片段、输出 HLS init/播放列表，
/// 还是报告诊断。
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
///
/// Bounded structured diagnostics for unsupported codecs, malformed frames,
/// backpressure drops, and files that should be discarded because they are too
/// small to be valid.
///
/// 录制写入器发出的诊断。
///
/// 针对不支持的编解码器、畸形帧、反压丢弃以及因过小而应丢弃文件的
/// 有界结构化诊断。
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
///
/// Invalid track sets, rejected frames, use-after-finalize, and internal
/// writer failures are surfaced as typed errors so the record module can
/// decide whether to abort or continue.
///
/// 录制写入器产生的错误。
///
/// 无效轨道集、被拒绝帧、finalized 后继续使用以及写入器内部失败
/// 都以类型化错误呈现，以便录制模块决定中止或继续。
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
///
/// Implementors receive track info, ingest `AVFrame`s, and emit `RecordWriteEvent`s
/// that the runtime layer persists to disk. The trait is `Send` so writers can be
/// moved between runtime tasks.
///
/// 容器无关的录制写入器 trait。
///
/// 实现者接收轨道信息、摄入 `AVFrame` 并发出 `RecordWriteEvent`，
/// 由运行时层持久化到磁盘。该 trait 为 `Send`，因此写入器可在运行时任务间移动。
pub trait RecordContainerWriter: Send {
    /// Update the active track set. Called once after opening; may be called
    /// again if upstream re-emits track info (re-sync).
    ///
    /// 更新活动轨道集。打开后调用一次；若上游再次发出轨道信息（重新同步）
    /// 可再次调用。
    fn update_tracks(&mut self, tracks: &[TrackInfo]) -> Result<(), RecordError>;

    /// Push an `AVFrame` into the writer. Returns 0 or more events.
    ///
    /// 将一帧 `AVFrame` 推入写入器，返回 0 个或多个事件。
    fn push_frame(&mut self, frame: &AVFrame) -> Result<Vec<RecordWriteEvent>, RecordError>;

    /// Finalize the writer; flushes any pending data and emits trailing
    /// boxes/playlists.
    ///
    /// 完成写入；刷新所有待处理数据并输出尾部 Box/播放列表。
    fn finalize(&mut self) -> Result<Vec<RecordWriteEvent>, RecordError>;

    /// Container format the writer produces.
    ///
    /// 写入器生成的容器格式。
    fn format(&self) -> RecordFormat;
}

/// Boxed writer alias the record module uses to dispatch by format.
///
/// 录制模块按格式分派使用的 boxed 写入器别名。
pub type DynRecordWriter = Box<dyn RecordContainerWriter>;

/// Build a default writer for the given format. This is a convenience for
/// testing; production callers may pass a custom config to the per-format
/// writer constructors directly.
///
/// 为给定格式构建默认写入器。便于测试；生产调用方可直接传入自定义配置
/// 给各格式写入器构造函数。
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
