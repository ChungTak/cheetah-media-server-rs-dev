//! `cheetah-rtsp-module`: Engine integration for the RTSP protocol.
//!
//! `cheetah-rtsp-module`：RTSP 协议引擎集成模块。

/// RTSP module configuration.
///
/// RTSP 模块配置。
pub mod config;
pub(crate) mod media;
/// RTSP module lifecycle, client/server HTTP API, and request dispatch.
///
/// RTSP 模块生命周期、客户端/服务端 HTTP API 与请求分发。
pub mod module;
/// RTSP runtime pull connector used by the high-level SDK.
///
/// 供高层 SDK 使用的 RTSP 拉流运行时连接器。
pub mod pull;
pub(crate) mod sdp;
pub(crate) mod session;

pub use config::RtspModuleConfig;
pub use module::RtspModuleFactory;
