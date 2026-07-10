//! `cheetah-fmp4-module`: Engine integration for fMP4 protocol.

/// Module for `config`.
/// `config` 相关模块。
pub mod config;
/// Module for `module`.
/// `module` 相关模块。
pub mod module;

pub use config::Fmp4ModuleConfig;
pub use module::Fmp4ModuleFactory;
