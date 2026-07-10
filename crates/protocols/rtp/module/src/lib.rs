//! `cheetah-rtp-module`: Engine integration for RTP protocol.
//!
//! `cheetah-rtp-module`：RTP 协议引擎集成模块。

/// RTP module configuration.
///
/// RTP 模块配置。
pub mod config;
/// RTP module lifecycle, HTTP control API, ingress/egress, and pull jobs.
///
/// RTP 模块生命周期、HTTP 控制 API、入站/出站与拉流任务。
pub mod module;

pub use config::RtpModuleConfig;
pub use module::RtpModuleFactory;
