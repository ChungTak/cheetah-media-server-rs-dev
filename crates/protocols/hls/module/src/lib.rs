//! HTTP Live Streaming (HLS) module integration crate.
//!
//! This crate wires the HLS core Sans-I/O state machine and the Tokio driver into the
//! engine module lifecycle, manages HTTP control endpoints, and handles publish/subscribe
//! interaction, TS/fMP4 muxing, and pull jobs.
//!
//! HTTP Live Streaming（HLS）模块集成 crate。
//!
//! 本 crate 将 HLS 核心无 I/O 状态机与 Tokio 驱动接入引擎模块生命周期，管理 HTTP
//! 控制端点，处理发布/订阅交互、TS/fMP4 复用以及拉流任务。
//!

/// HLS module configuration and default values.
///
/// HLS 模块配置与默认值。
pub mod config;
/// Demuxed audio/video LL-HLS muxer.
///
/// 分离音视频轨道的 LL-HLS 复用器。
pub(crate) mod demuxed_muxer;
/// HLS module lifecycle and HTTP server loop.
///
/// HLS 模块生命周期与 HTTP 服务循环。
pub mod module;
/// Per-stream TS/fMP4 muxer.
///
/// 每个流的 TS/fMP4 复用器。
pub(crate) mod muxer;
/// HLS pull job implementation.
///
/// HLS 拉流任务实现。
pub(crate) mod pull;
/// HLS playback statistics.
///
/// HLS 播放统计。
pub mod stats;
/// Per-track fMP4 muxer for demuxed LL-HLS.
///
/// demuxed LL-HLS 的每轨 fMP4 复用器。
pub(crate) mod track_muxer;

/// Re-exported top-level HLS module configuration.
///
/// 重导出的顶层 HLS 模块配置。
pub use config::HlsModuleConfig;
/// Re-exported HLS module factory.
///
/// 重导出的 HLS 模块工厂。
pub use module::HlsModuleFactory;
/// Re-exported HLS stream and session statistics.
///
/// 重导出的 HLS 流和会话统计信息。
pub use stats::{HlsSessionInfo, HlsStreamStats};
