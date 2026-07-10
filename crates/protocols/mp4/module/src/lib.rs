//! MP4 VOD module — engine integration, REST API, session lifecycle.

/// Module for `api`.
/// `api` 相关模块。
pub mod api;
/// Module for `config`.
/// `config` 相关模块。
pub mod config;
/// Module for `module`.
/// `module` 相关模块。
pub mod module;
/// Module for `session_registry`.
/// `session_registry` 相关模块。
pub mod session_registry;
/// Module for `zlm_compat`.
/// `zlm_compat` 相关模块。
pub mod zlm_compat;

pub use api::VodApi;
pub use config::Mp4ModuleConfig;
pub use module::{Mp4Module, Mp4ModuleFactory};
pub use session_registry::{VodSessionRecord, VodSessionRegistry};
pub use zlm_compat::{
    expand_uri_list, normalize_rtmp_mp4_uri, ZlmLoadMp4, ZlmSeekRecord, ZlmSetSpeed, ZlmVodCompat,
    ZlmVodError,
};
