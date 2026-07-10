//! `cheetah-fmp4-module`: Engine integration for the fMP4 protocol.
//!
//! `cheetah-fmp4-module`：fMP4 协议的引擎集成。

/// fMP4 module configuration.
///
/// fMP4 模块配置。
pub mod config;
/// fMP4 module factory and `Module` implementation.
///
/// fMP4 模块工厂与 `Module` 实现。
pub mod module;

pub use config::Fmp4ModuleConfig;
pub use module::Fmp4ModuleFactory;
