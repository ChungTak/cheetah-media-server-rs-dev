//! `cheetah-gb28181-module`: Engine integration for GB28181 protocol.
//!
//! `cheetah-gb28181-module`：GB28181 协议引擎集成模块。

/// GB28181 module configuration.
///
/// GB28181 模块配置。
pub mod config;
/// GB28181 module lifecycle, HTTP API, and SIP/RTP coordination.
///
/// GB28181 模块生命周期、HTTP API 与 SIP/RTP 协调。
pub mod module;

pub use config::Gb28181ModuleConfig;
pub use module::{Gb28181Module, Gb28181ModuleFactory};
