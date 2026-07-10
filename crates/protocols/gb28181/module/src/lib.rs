/// `config` module.
/// `config` 模块.
pub mod config;
/// `module` module.
/// `模块` 模块.
pub mod module;

pub use config::Gb28181ModuleConfig;
pub use module::{Gb28181Module, Gb28181ModuleFactory};
