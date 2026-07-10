//! MP4 VOD module — engine integration, REST API, session lifecycle.

/// `api` module.
/// `api` 模块.
pub mod api;
/// `config` module.
/// `config` 模块.
pub mod config;
/// `module` module.
/// `模块` 模块.
pub mod module;
/// `session_registry` module.
/// `session_registry` 模块.
pub mod session_registry;
/// `zlm_compat` module.
/// `zlm_compat` 模块.
pub mod zlm_compat;

pub use api::VodApi;
pub use config::Mp4ModuleConfig;
pub use module::{Mp4Module, Mp4ModuleFactory};
pub use session_registry::{VodSessionRecord, VodSessionRegistry};
pub use zlm_compat::{
    expand_uri_list, normalize_rtmp_mp4_uri, ZlmLoadMp4, ZlmSeekRecord, ZlmSetSpeed, ZlmVodCompat,
    ZlmVodError,
};
