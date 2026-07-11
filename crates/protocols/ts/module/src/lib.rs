//! `cheetah-ts-module`: Engine integration for TS protocol.
//!
//! `cheetah-ts-module`：TS 协议引擎集成模块。

/// TS module configuration.
///
/// TS 模块配置。
pub mod config;
/// TS module lifecycle, play session, and pull job orchestration.
///
/// TS 模块生命周期、播放会话与拉流任务编排。
pub mod module;

pub use config::TsModuleConfig;
pub use module::TsModuleFactory;
