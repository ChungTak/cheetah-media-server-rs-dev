//! MP4 VOD module — engine integration, REST API, session lifecycle.
//!
//! MP4 VOD 模块 —— 引擎集成、REST API、会话生命周期。

/// VOD HTTP API request/response handlers.
///
/// VOD HTTP API 请求/响应处理器。
pub mod api;
/// MP4 VOD module configuration.
///
/// MP4 VOD 模块配置。
pub mod config;
/// `Module` integration and HTTP route registration.
///
/// `Module` 集成与 HTTP 路由注册。
pub mod module;
/// VOD session registry.
///
/// VOD 会话注册表。
pub mod session_registry;
/// ZLM-compatible VOD URI normalization and API shims.
///
/// 兼容 ZLM 的 VOD URI 规范化与 API 适配层。
pub mod zlm_compat;

pub use api::VodApi;
pub use config::Mp4ModuleConfig;
pub use module::{Mp4Module, Mp4ModuleFactory};
pub use session_registry::{VodSessionRecord, VodSessionRegistry};
pub use zlm_compat::{
    expand_uri_list, normalize_rtmp_mp4_uri, ZlmLoadMp4, ZlmSeekRecord, ZlmSetSpeed, ZlmVodCompat,
    ZlmVodError,
};
