//! Engine integration for SRT.

/// Module for `config`.
/// `config` 相关模块。
pub mod config;
mod http;
mod metrics;
/// Module for `module`.
/// `module` 相关模块。
pub mod module;

pub use config::SrtModuleConfig;
pub use metrics::{SrtModuleMetrics, SrtModuleMetricsSnapshot};
pub use module::SrtModuleFactory;
