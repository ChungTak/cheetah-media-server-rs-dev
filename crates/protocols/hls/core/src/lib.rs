//! `cheetah-hls-core`: Sans-I/O state machine for the HLS protocol.
//!
//! `cheetah-hls-core` 是 HLS 协议的 Sans-I/O 状态机。
//! 负责 M3U8 播放列表生成与解析、LL-HLS 片段、TS/fMP4 复用/解复用、
//! 分片环形缓冲、播放器状态与请求路由，不依赖任何运行时或套接字。

/// HLS core error types.
///
/// HLS core 错误类型。
pub mod error;
/// fMP4 demuxer for HLS pull playback.
///
/// 用于 HLS 拉流播放的 fMP4 解复用器。
pub mod fmp4_demux;
/// fMP4 muxer for HLS segment generation.
///
/// 用于 HLS 分片生成的 fMP4 复用器。
pub mod fmp4_mux;
/// Low-Latency HLS (LL-HLS) state and playlist tag generation.
///
/// 低延迟 HLS（LL-HLS）状态与播放列表标签生成。
pub mod ll_hls;
/// SCTE-35 style CUE marker support.
///
/// SCTE-35 风格 CUE 标记支持。
pub mod marker;
/// HLS playback pacing and timestamp smoothing.
///
/// HLS 播放节奏控制与 timestamp 平滑。
pub mod pacer;
/// M3U8 playlist parser.
///
/// M3U8 播放列表解析器。
pub mod parser;
/// HLS player state and adaptive bitrate selection.
///
/// HLS 播放器状态与自适应码率选择。
pub mod player;
/// M3U8 playlist builder.
///
/// M3U8 播放列表构建器。
pub mod playlist;
/// HLS request routing and query parsing.
///
/// HLS 请求路由与查询参数解析。
pub mod request;
/// In-memory segment ring buffer.
///
/// 内存中的分片环形缓冲。
pub mod segment;
/// HLS core HTTP session state machine.
///
/// HLS core HTTP 会话状态机。
pub mod session;
/// MPEG-TS demuxer for HLS pull playback.
///
/// 用于 HLS 拉流播放的 MPEG-TS 解复用器。
pub mod ts_demux;
/// MPEG-TS muxer for HLS segment generation.
///
/// 用于 HLS 分片生成的 MPEG-TS 复用器。
pub mod ts_mux;
/// WebVTT segment muxer for HLS subtitles.
///
/// HLS 字幕 WebVTT 分片复用器。
pub mod vtt_mux;

pub use error::HlsCoreError;
pub use fmp4_demux::{Fmp4DemuxEvent, Fmp4DemuxTrack, Fmp4Demuxer};
pub use fmp4_mux::{Fmp4Muxer, Fmp4Sample, Fmp4TrackDesc};
pub use ll_hls::{
    HlsPart, LlHlsPackagingMode, LowLatencyState, RenditionReport, SegmentParts, TrackLane,
};
pub use marker::{format_cue_tags, CueMarker};
pub use pacer::{HlsPlaybackPacer, PacedFrame, StampSmoother};
pub use parser::{
    parse_master_playlist, parse_media_playlist, ParsedMasterPlaylist, ParsedMediaPlaylist,
    ParsedSegment, ParsedVariant,
};
pub use player::{BandwidthStrategy, HlsPlayerState};
pub use playlist::{
    format_iso8601, DemuxedMasterPlaylist, HlsContainer, MediaRenditionInfo, PlaylistBuilder,
    SegmentFileEntry, SubtitleRenditionInfo, VariantRenditionInfo,
};
pub use request::{parse_hls_request, BlockingParams, HlsRequestKind, SkipMode, StreamKeyParts};
pub use segment::{Segment, SegmentRing};
pub use session::{
    HlsCore, HlsCoreCommand, HlsCoreEvent, HlsCoreInput, HlsCoreOutput, HlsRequestHeaders,
    HlsSessionId, HttpMethod,
};
pub use ts_demux::{TsDemuxEvent, TsDemuxer};
pub use ts_mux::TsMuxer;
pub use vtt_mux::{VttMux, VttMuxConfig, VttSegment};
