//! PS demuxer diagnostics and events.
//!
//! PS 解复用器诊断与事件。

use crate::frame::AVFrame;
use crate::prelude::*;
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
    /// Maximum size of a single PES packet, including the 6-byte PES header.
    ///
    /// 单个 PES 包的最大大小，包含 6 字节 PES 头。
    pub max_pes_packet_size: usize,
    /// Maximum size of an assembled video access unit.
    ///
    /// 组装后的视频访问单元最大大小。
    pub max_access_unit_size: usize,
    /// Maximum number of PS pack headers to inspect before a track is found.
    ///
    /// 在发现轨道前允许检查的最大 PS pack header 数量。
    pub max_probe_packets: u32,
}

impl PsDemuxerConfig {
    /// Create a new configuration with the specified reassembly and track limits,
    /// using the defaults for all other limits.
    ///
    /// 使用指定的重组和轨道限制创建新配置，其他限制使用默认值。
    pub fn new(max_reassembly_bytes: usize, max_tracks: usize) -> Self {
        Self {
            max_reassembly_bytes,
            max_tracks,
            ..Default::default()
        }
    }
}

impl Default for PsDemuxerConfig {
    fn default() -> Self {
        Self {
            max_reassembly_bytes: 4 * 1024 * 1024,
            max_tracks: 32,
            max_pes_packet_size: 8 * 1024 * 1024,
            max_access_unit_size: 16 * 1024 * 1024,
            max_probe_packets: 1024,
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
    /// A configured per-session limit was exceeded and state was cleared.
    ///
    /// 配置的每 session 限制被超过并已清理状态。
    LimitExceeded { resource: String },
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
