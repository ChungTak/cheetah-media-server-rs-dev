//! `cheetah-fmp4-module`: Engine integration for fMP4 protocol.

/// `config` module.
/// `config` 模块.
pub mod config;
/// `module` module.
/// `模块` 模块.
pub mod module;

pub use config::Fmp4ModuleConfig;
pub use module::Fmp4ModuleFactory;
