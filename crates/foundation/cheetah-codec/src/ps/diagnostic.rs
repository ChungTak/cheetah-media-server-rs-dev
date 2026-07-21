//! PS demuxer diagnostics and events.
//!
//! PS 解复用器诊断与事件。

use crate::prelude::*;
use crate::frame::AVFrame;
use crate::track::TrackInfo;

/// Configuration for the PS demuxer.
///
/// PS 解复用器配置。
#[derive(Debug, Clone)]
pub struct PsDemuxerConfig {
    /// Maximum bytes retained in the reassembly buffer.
    ///
    /// 重组缓冲区允许保留的最大字节数。
    pub max_reassembly_bytes: usize,
    /// Maximum number of tracks to retain.
    ///
    /// 允许保留的最大轨道数。
    pub max_tracks: usize,
}

impl Default for PsDemuxerConfig {
    fn default() -> Self {
        Self {
            max_reassembly_bytes: 4 * 1024 * 1024,
            max_tracks: 32,
        }
    }
}

/// Diagnostic events emitted by the PS demuxer.
///
/// PS 解复用器发出的诊断事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsDemuxDiagnostic {
    /// Reassembly buffer exceeded the configured limit.
    ///
    /// 重组缓冲区超过配置上限。
    BufferOverflow,
    /// A start code was not followed by a valid stream ID.
    ///
    /// 起始码后未跟随有效的流 ID。
    InvalidStartCode { code: u8 },
    /// Program Stream Map (PSM) parsing failed.
    ///
    /// 节目流映射（PSM）解析失败。
    PsmParseError,
    /// PES packet parsing failed.
    ///
    /// PES 包解析失败。
    PesParseError,
}

/// Events produced by the PS demuxer.
///
/// PS 解复用器产生的事件。
#[derive(Debug, Clone)]
pub enum PsDemuxEvent {
    /// One or more discovered tracks.
    ///
    /// 发现的一个或多个轨道。
    TrackInfo(Vec<TrackInfo>),
    /// A completed media frame.
    ///
    /// 一个完整的媒体帧。
    Frame(Box<AVFrame>),
    /// A diagnostic event.
    ///
    /// 一次诊断事件。
    Diagnostic(PsDemuxDiagnostic),
}
