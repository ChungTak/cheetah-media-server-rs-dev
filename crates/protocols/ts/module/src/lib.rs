//! `cheetah-ts-module`: Engine integration for TS protocol.

/// Module for `config`.
/// `config` 相关模块。
pub mod config;
/// Module for `module`.
/// `module` 相关模块。
pub mod module;

pub use config::TsModuleConfig;
pub use module::TsModuleFactory;
