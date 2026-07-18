//! RTMP protocol module integration for the Cheetah engine.
//!
//! This crate wires the Sans-I/O RTMP core and the Tokio driver into the engine
//! lifecycle: configuration, server lifecycle, HTTP route wiring, ingest/egress,
//! and publish/play session management.
//!
//! Cheetah 引擎的 RTMP 协议模块集成。
//!
//! 本 crate 将无 I/O 的 RTMP 核心与 Tokio 驱动接入引擎生命周期：
//! 配置、服务生命周期、HTTP 路由接入、输入/输出以及发布/播放会话管理。

pub mod config;
/// RTMP egress logic: maps internal frames to RTMP/FLV commands and bootstraps play streams.
///
/// RTMP 输出逻辑：将内部帧映射为 RTMP/FLV 命令并起播。
pub(crate) mod egress;
/// RTMP ingest logic: parses video/audio/data tags and normalizes timestamps.
///
/// RTMP 输入逻辑：解析视频/音频/数据标签并归一化时间戳。
pub(crate) mod ingest;
/// RTMP module lifecycle, HTTP routes, session management, and integration.
///
/// RTMP 模块生命周期、HTTP 路由、会话管理与集成。
pub mod module;
/// NAL unit helpers: extracts and validates H.264/H.265 NAL length sizes.
///
/// NAL 单元辅助函数：提取并校验 H.264/H.265 的 NAL 长度大小。
pub(crate) mod nal;
/// RTMP processing helpers: derived AAC/H.264 streams.
///
/// RTMP 处理辅助函数：派生 AAC/H.264 流。
pub(crate) mod processing;
/// RTMP stream route parsing and play mode selection.
///
/// RTMP 流路由解析与播放模式选择。
pub(crate) mod route;
/// RTMP publish/play session state and timestamp bookkeeping.
///
/// RTMP 发布/播放会话状态与时间戳簿记。
pub(crate) mod session;

/// Re-exported RTMP module configuration.
///
/// 重新导出的 RTMP 模块配置。
pub use config::RtmpModuleConfig;
/// Re-exported RTMP module factory.
///
/// 重新导出的 RTMP 模块工厂。
pub use module::RtmpModuleFactory;
