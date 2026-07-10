//! MP4 VOD module — engine integration, REST API, session lifecycle.

pub mod api;
pub mod config;
pub mod module;
pub mod session_registry;
pub mod zlm_compat;

pub use api::VodApi;
pub use config::Mp4ModuleConfig;
pub use module::{Mp4Module, Mp4ModuleFactory};
pub use session_registry::{VodSessionRecord, VodSessionRegistry};
pub use zlm_compat::{
    expand_uri_list, normalize_rtmp_mp4_uri, ZlmLoadMp4, ZlmSeekRecord, ZlmSetSpeed, ZlmVodCompat,
    ZlmVodError,
};
