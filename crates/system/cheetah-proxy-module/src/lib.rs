//! Proxy module for external pull/push/FFmpeg bridging.
//!
//! 用于外部拉流、推流、FFmpeg 桥接的代理模块。

pub mod config;
pub mod media_provider;
pub mod module;
pub mod registry;

pub use module::ProxyModuleFactory;
