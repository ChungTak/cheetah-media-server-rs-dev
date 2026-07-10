//! `cheetah-ts-module`: Engine integration for TS protocol.

/// `config` module.
/// `config` 模块.
pub mod config;
/// `module` module.
/// `模块` 模块.
pub mod module;

pub use config::TsModuleConfig;
pub use module::TsModuleFactory;
