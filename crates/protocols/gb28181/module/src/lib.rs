/// Module for `config`.
/// `config` 相关模块。
pub mod config;
/// Module for `module`.
/// `module` 相关模块。
pub mod module;

pub use config::Gb28181ModuleConfig;
pub use module::{Gb28181Module, Gb28181ModuleFactory};
